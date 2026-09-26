//! Minimal glue for embedding a headless Bevy app inside a gpui window.
//!
//! gpui owns the window and the event loop. Bevy runs without winit, renders its
//! camera into an offscreen image, and the pixels are read back to the CPU and
//! shown in gpui as an `img()` element.
//!
//! ```text
//! gpui timer ──► EmbeddedBevy::update() ──► Bevy renders to Image
//!                                              │ Readback (GPU → CPU)
//! gpui render ◄── ViewportImage::sync() ◄──── latest BGRA frame
//! ```

use std::sync::{Arc, Mutex};

use bevy::{
    app::{App as BevyApp, PluginsState},
    camera::RenderTarget,
    prelude::*,
    render::{
        gpu_readback::{Readback, ReadbackComplete},
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{Extent3d, TextureFormat, TextureUsages},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use gpui::RenderImage;

/// Marks the camera whose output is shown in the gpui viewport.
#[derive(Component)]
pub struct ViewportCamera;

/// One CPU-side frame in BGRA8 (sRGB) with tightly packed rows.
pub struct CpuFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// The offscreen image the viewport camera renders into.
#[derive(Resource)]
struct ViewportTarget {
    image: Handle<Image>,
    size: UVec2,
}

/// Latest frame written by the readback observer and taken by gpui.
#[derive(Clone, Default)]
struct LatestFrame(Arc<Mutex<Option<CpuFrame>>>);

/// A Bevy app running without a window, driven manually from gpui.
pub struct EmbeddedBevy {
    app: BevyApp,
    latest: LatestFrame,
}

impl EmbeddedBevy {
    /// Builds a headless Bevy app. `configure` adds the game/scene plugins and
    /// must spawn a camera with [`ViewportCamera`].
    pub fn new(size: UVec2, configure: impl FnOnce(&mut BevyApp)) -> Self {
        let mut app = BevyApp::new();
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                })
                .disable::<WinitPlugin>()
                // Keep rendering on the calling thread so `update()` is synchronous.
                .disable::<PipelinedRenderingPlugin>(),
        );

        let latest = LatestFrame::default();
        app.add_systems(PostUpdate, attach_target_to_camera);
        configure(&mut app);

        // What `App::run` normally does before the first update.
        while app.plugins_state() == PluginsState::Adding {
            bevy::tasks::tick_global_task_pools_on_main_thread();
        }
        app.finish();
        app.cleanup();
        create_target(app.world_mut(), size, latest.clone());

        Self { app, latest }
    }

    /// Runs one Bevy frame (simulation + render + readback submission).
    pub fn update(&mut self) {
        self.app.update();
    }

    pub fn world(&self) -> &World {
        self.app.world()
    }

    pub fn world_mut(&mut self) -> &mut World {
        self.app.world_mut()
    }

    /// Returns the newest frame read back from the GPU, if any arrived since the last call.
    pub fn take_frame(&self) -> Option<CpuFrame> {
        self.latest.0.lock().unwrap().take()
    }

    pub fn size(&self) -> UVec2 {
        self.world().resource::<ViewportTarget>().size
    }

    /// Resizes the offscreen image. Bevy recreates the GPU texture on the next update.
    pub fn resize(&mut self, size: UVec2) {
        let size = size.max(UVec2::ONE);
        let world = self.app.world_mut();
        let mut target = world.resource_mut::<ViewportTarget>();
        if target.size == size {
            return;
        }
        target.size = size;
        let handle = target.image.clone();
        let mut images = world.resource_mut::<Assets<Image>>();
        let mut image = images.get_mut(&handle).expect("viewport image exists");
        image.resize(Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        });
    }
}

fn create_target(world: &mut World, size: UVec2, latest: LatestFrame) {
    let mut image = Image::new_target_texture(
        size.x,
        size.y,
        // gpui's RenderImage expects BGRA, so no swizzle is needed after readback.
        TextureFormat::Bgra8UnormSrgb,
        None,
    );
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let image = world.resource_mut::<Assets<Image>>().add(image);

    world.insert_resource(ViewportTarget {
        image: image.clone(),
        size,
    });

    // Readback runs every frame while this component exists.
    world.spawn(Readback::texture(image)).observe(
        move |event: On<ReadbackComplete>, target: Res<ViewportTarget>| {
            if let Some(frame) = unpad(&event.data, target.size) {
                *latest.0.lock().unwrap() = Some(frame);
            }
        },
    );
}

fn attach_target_to_camera(
    mut commands: Commands,
    target: Res<ViewportTarget>,
    cameras: Query<Entity, Added<ViewportCamera>>,
) {
    for camera in &cameras {
        commands
            .entity(camera)
            .insert(RenderTarget::Image(target.image.clone().into()));
    }
}

/// Readback rows are padded to 256 bytes; strip the padding.
/// Returns `None` when the data belongs to a different size (e.g. mid-resize).
fn unpad(data: &[u8], size: UVec2) -> Option<CpuFrame> {
    let row = size.x as usize * 4;
    let padded_row = row.next_multiple_of(256);
    if data.len() != padded_row * size.y as usize {
        return None;
    }
    let mut bgra = Vec::with_capacity(row * size.y as usize);
    for chunk in data.chunks_exact(padded_row) {
        bgra.extend_from_slice(&chunk[..row]);
    }
    Some(CpuFrame {
        width: size.x,
        height: size.y,
        bgra,
    })
}

/// Holds the gpui-side image for the viewport and frees old frames from gpui's atlas.
#[derive(Default)]
pub struct ViewportImage {
    current: Option<Arc<RenderImage>>,
}

impl ViewportImage {
    /// Swaps in `frame` (if any) and drops the previous image from the sprite atlas.
    pub fn sync(&mut self, frame: Option<CpuFrame>, window: &mut gpui::Window) {
        let Some(frame) = frame else { return };
        let buffer = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
            .expect("buffer size matches dimensions");
        let new = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
            buffer
        )]));
        if let Some(old) = self.current.replace(new) {
            window.drop_image(old).ok();
        }
    }

    pub fn current(&self) -> Option<Arc<RenderImage>> {
        self.current.clone()
    }
}
