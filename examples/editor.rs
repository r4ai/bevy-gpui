//! Use case 2: a Blender-like editor. Bevy renders the 3D viewport, gpui draws
//! the outliner and properties panels around it.
//!
//! Drag in the viewport to orbit, scroll to zoom. Click an object in the
//! outliner to select it, then recolor or delete it in the properties panel.

use std::time::Duration;

use bevy::prelude as bv;
use bevy::prelude::{
    Assets, Color, Commands, Component, Gizmos, Mesh, Meshable, Name, Query, Res, ResMut, Resource,
    StandardMaterial, Transform, UVec2, Vec3, With,
};
use bevy_gpui::{EmbeddedBevy, ViewportCamera, ViewportImage};
use gpui::{
    App, Bounds, Context, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels,
    Point, ScrollWheelEvent, SharedString, Window, WindowBounds, WindowOptions, div, img,
    prelude::*, px, rgb, size,
};

const OUTLINER_WIDTH: f32 = 220.;
const PROPERTIES_WIDTH: f32 = 240.;

// ---------------------------------------------------------------------------
// Bevy side: the scene and the orbit camera.
// ---------------------------------------------------------------------------

/// Objects the user can see and edit in the outliner.
#[derive(Component)]
struct SceneObject;

/// The currently selected object (at most one).
#[derive(Component)]
struct Selected;

#[derive(Resource)]
struct OrbitCamera {
    yaw: f32,
    pitch: f32,
    distance: f32,
}

fn scene_plugin(app: &mut bv::App) {
    app.insert_resource(OrbitCamera {
        yaw: 0.6,
        pitch: 0.5,
        distance: 10.0,
    })
    .add_systems(bv::Startup, setup)
    .add_systems(bv::Update, (apply_orbit, draw_selection));
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        bv::Camera3d::default(),
        ViewportCamera,
        Transform::default(),
    ));
    commands.spawn((
        bv::DirectionalLight {
            shadow_maps_enabled: true,
            ..bv::default()
        },
        Transform::from_xyz(4.0, 10.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        bv::Mesh3d(meshes.add(bv::Plane3d::default().mesh().size(12.0, 12.0))),
        bv::MeshMaterial3d(materials.add(Color::srgb(0.35, 0.35, 0.38))),
    ));
    commands.spawn((
        SceneObject,
        Name::new("Cube"),
        bv::Mesh3d(meshes.add(bv::Cuboid::from_length(1.0))),
        bv::MeshMaterial3d(materials.add(Color::srgb(0.8, 0.8, 0.8))),
        Transform::from_xyz(0.0, 0.5, 0.0),
    ));
    commands.spawn((
        SceneObject,
        Name::new("Sphere"),
        bv::Mesh3d(meshes.add(bv::Sphere::new(0.6))),
        bv::MeshMaterial3d(materials.add(Color::srgb(0.8, 0.3, 0.3))),
        Transform::from_xyz(2.0, 0.6, -1.0),
    ));
}

fn apply_orbit(orbit: Res<OrbitCamera>, mut cameras: Query<&mut Transform, With<ViewportCamera>>) {
    let offset = Vec3::new(
        orbit.pitch.cos() * orbit.yaw.sin(),
        orbit.pitch.sin(),
        orbit.pitch.cos() * orbit.yaw.cos(),
    ) * orbit.distance;
    for mut transform in &mut cameras {
        *transform = Transform::from_translation(offset).looking_at(Vec3::ZERO, Vec3::Y);
    }
}

fn draw_selection(mut gizmos: Gizmos, selected: Query<&Transform, With<Selected>>) {
    for transform in &selected {
        gizmos.cube(
            transform.with_scale(Vec3::splat(1.3)),
            Color::srgb(1.0, 0.6, 0.0),
        );
    }
}

// ---------------------------------------------------------------------------
// gpui side: panels + viewport. All edits go straight into the Bevy World.
// ---------------------------------------------------------------------------

struct Editor {
    bevy: EmbeddedBevy,
    viewport: ViewportImage,
    last_drag: Option<Point<Pixels>>,
    cubes_added: u32,
}

impl Editor {
    fn new(cx: &mut Context<Self>) -> Self {
        let bevy = EmbeddedBevy::new(UVec2::new(800, 600), scene_plugin);

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    this.bevy.update();
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        Self {
            bevy,
            viewport: ViewportImage::default(),
            last_drag: None,
            cubes_added: 0,
        }
    }

    /// Keeps the Bevy render target the same physical size as the viewport panel.
    fn fit_viewport(&mut self, window: &Window) {
        let window_size = window.viewport_size();
        let scale = window.scale_factor();
        let width = (f32::from(window_size.width) - OUTLINER_WIDTH - PROPERTIES_WIDTH) * scale;
        let height = f32::from(window_size.height) * scale;
        self.bevy
            .resize(UVec2::new(width.max(1.) as u32, height.max(1.) as u32));
    }

    fn objects(&mut self) -> Vec<(bv::Entity, String, bool)> {
        let world = self.bevy.world_mut();
        let mut query =
            world.query_filtered::<(bv::Entity, &Name, bv::Has<Selected>), With<SceneObject>>();
        let mut objects: Vec<_> = query
            .iter(world)
            .map(|(entity, name, selected)| (entity, name.to_string(), selected))
            .collect();
        objects.sort_by_key(|(entity, ..)| *entity);
        objects
    }

    fn selected(&mut self) -> Option<(bv::Entity, String, Vec3)> {
        let world = self.bevy.world_mut();
        let mut query = world.query_filtered::<(bv::Entity, &Name, &Transform), With<Selected>>();
        query
            .iter(world)
            .next()
            .map(|(entity, name, transform)| (entity, name.to_string(), transform.translation))
    }

    fn select(&mut self, entity: bv::Entity) {
        let world = self.bevy.world_mut();
        let mut query = world.query_filtered::<bv::Entity, With<Selected>>();
        let previous: Vec<_> = query.iter(world).collect();
        for previous in previous {
            world.entity_mut(previous).remove::<Selected>();
        }
        world.entity_mut(entity).insert(Selected);
    }

    fn add_cube(&mut self) {
        self.cubes_added += 1;
        let n = self.cubes_added;
        let world = self.bevy.world_mut();
        let mesh = world
            .resource_mut::<Assets<Mesh>>()
            .add(bv::Cuboid::from_length(1.0));
        let material = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(Color::srgb(0.3, 0.6, 0.9));
        let entity = world
            .spawn((
                SceneObject,
                Name::new(format!("Cube.{n:03}")),
                bv::Mesh3d(mesh),
                bv::MeshMaterial3d(material),
                Transform::from_xyz(-2.0 + (n % 5) as f32, 0.5, 2.0 - (n / 5) as f32),
            ))
            .id();
        self.select(entity);
    }

    fn set_color(&mut self, entity: bv::Entity, color: Color) {
        let world = self.bevy.world_mut();
        let handle = world
            .get::<bv::MeshMaterial3d<StandardMaterial>>(entity)
            .expect("scene objects have a material")
            .0
            .clone();
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        if let Some(mut material) = materials.get_mut(&handle) {
            material.base_color = color;
        }
    }

    fn on_viewport_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.last_drag = Some(event.position);
    }

    fn on_viewport_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        if event.pressed_button != Some(MouseButton::Left) {
            self.last_drag = None;
            return;
        }
        let Some(last) = self.last_drag.replace(event.position) else {
            return;
        };
        let delta = event.position - last;
        let mut orbit = self.bevy.world_mut().resource_mut::<OrbitCamera>();
        orbit.yaw -= f32::from(delta.x) * 0.01;
        orbit.pitch = (orbit.pitch + f32::from(delta.y) * 0.01).clamp(0.05, 1.5);
    }

    fn on_viewport_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        let dy = f32::from(event.delta.pixel_delta(px(16.)).y);
        let mut orbit = self.bevy.world_mut().resource_mut::<OrbitCamera>();
        orbit.distance = (orbit.distance - dy * 0.02).clamp(2.0, 40.0);
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.fit_viewport(window);
        self.viewport.sync(self.bevy.take_frame(), window);

        let outliner = panel(OUTLINER_WIDTH)
            .child(panel_title("Outliner"))
            .child(
                button("add-cube", "+ Add Cube").on_click(cx.listener(|this, _, _, cx| {
                    this.add_cube();
                    cx.notify();
                })),
            )
            .children(self.objects().into_iter().map(|(entity, name, selected)| {
                div()
                    .id(("object", entity.to_bits()))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(selected, |row| row.bg(rgb(0x3d5afe)))
                    .when(!selected, |row| row.hover(|style| style.bg(rgb(0x3a3a40))))
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(entity);
                        cx.notify();
                    }))
            }));

        let properties = panel(PROPERTIES_WIDTH)
            .child(panel_title("Properties"))
            .child(match self.selected() {
                None => div().text_color(rgb(0x888888)).child("Nothing selected"),
                Some((entity, name, position)) => div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_lg().child(name))
                    .child(format!(
                        "Location  X {:.2}  Y {:.2}  Z {:.2}",
                        position.x, position.y, position.z
                    ))
                    .child("Color")
                    .child(
                        div().flex().gap_2().children(
                            [
                                ("red", 0xe53935, Color::srgb(0.9, 0.2, 0.2)),
                                ("green", 0x43a047, Color::srgb(0.25, 0.65, 0.3)),
                                ("blue", 0x1e88e5, Color::srgb(0.1, 0.5, 0.9)),
                                ("white", 0xeeeeee, Color::srgb(0.9, 0.9, 0.9)),
                            ]
                            .map(|(id, swatch, color)| {
                                div()
                                    .id(id)
                                    .size_6()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(0x000000))
                                    .bg(rgb(swatch))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_color(entity, color);
                                        cx.notify();
                                    }))
                            }),
                        ),
                    )
                    .child(button("delete", "Delete").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.bevy.world_mut().despawn(entity);
                            cx.notify();
                        },
                    ))),
            });

        let viewport = div()
            .id("viewport")
            .flex_1()
            .h_full()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_viewport_mouse_down))
            .on_mouse_move(cx.listener(Self::on_viewport_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_viewport_scroll))
            .children(
                self.viewport
                    .current()
                    .map(|image| img(image).size_full().object_fit(ObjectFit::Fill)),
            );

        div()
            .flex()
            .size_full()
            .bg(rgb(0x1e1e22))
            .text_color(rgb(0xdddddd))
            .text_sm()
            .child(outliner)
            .child(viewport)
            .child(properties)
    }
}

fn panel(width: f32) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .w(px(width))
        .h_full()
        .flex_none()
        .bg(rgb(0x2a2a2e))
}

fn panel_title(title: &'static str) -> impl IntoElement {
    div()
        .pb_1()
        .mb_1()
        .border_b_1()
        .border_color(Hsla::from(rgb(0x444444)))
        .text_color(rgb(0xaaaaaa))
        .child(SharedString::from(title))
}

fn button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(0x44444c))
        .hover(|style| style.bg(rgb(0x55555e)))
        .cursor_pointer()
        .child(label)
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        let bounds = Bounds::centered(None, size(px(1280.), px(760.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(Editor::new),
        )
        .unwrap();
        cx.activate(true);
    });
}
