//! Use case 1: a Bevy game whose HUD and menus are drawn by gpui.
//!
//! WASD moves the ball, collect the gold cube. Esc toggles the pause menu.

use std::time::{Duration, Instant};

use bevy::prelude as bv;
use bevy::prelude::{
    Assets, ButtonInput, Color, Commands, Component, IntoScheduleConfigs, KeyCode, Mesh, Meshable,
    Query, Res, ResMut, Resource, StandardMaterial, Time, Transform, UVec2, Vec3, With, Without,
};
use bevy_gpui::{EmbeddedBevy, ViewportCamera, ViewportImage};
use gpui::{
    App, Bounds, Context, FocusHandle, KeyDownEvent, KeyUpEvent, ObjectFit, SharedString, Window,
    WindowBounds, WindowOptions, div, img, prelude::*, px, rgb, rgba, size,
};

// ---------------------------------------------------------------------------
// Bevy side: an ordinary Bevy game that knows nothing about gpui.
// ---------------------------------------------------------------------------

#[derive(Component)]
struct Player;

#[derive(Component)]
struct Coin;

#[derive(Resource, Default)]
struct Score(u32);

#[derive(Resource, Default)]
struct Paused(bool);

fn game_plugin(app: &mut bv::App) {
    app.init_resource::<Score>()
        .init_resource::<Paused>()
        .add_systems(bv::Startup, setup)
        .add_systems(
            bv::Update,
            (move_player, spin_coin, collect_coin).run_if(|paused: Res<Paused>| !paused.0),
        );
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        bv::Camera3d::default(),
        ViewportCamera,
        Transform::from_xyz(0.0, 12.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        bv::DirectionalLight {
            shadow_maps_enabled: true,
            ..bv::default()
        },
        Transform::from_xyz(4.0, 10.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        bv::Mesh3d(meshes.add(bv::Plane3d::default().mesh().size(20.0, 20.0))),
        bv::MeshMaterial3d(materials.add(Color::srgb(0.3, 0.5, 0.3))),
    ));
    commands.spawn((
        Player,
        bv::Mesh3d(meshes.add(bv::Sphere::new(0.5))),
        bv::MeshMaterial3d(materials.add(Color::srgb(0.2, 0.4, 0.9))),
        Transform::from_xyz(0.0, 0.5, 0.0),
    ));
    commands.spawn((
        Coin,
        bv::Mesh3d(meshes.add(bv::Cuboid::from_length(0.6))),
        bv::MeshMaterial3d(materials.add(Color::srgb(1.0, 0.8, 0.1))),
        Transform::from_translation(coin_position(0)),
    ));
}

fn move_player(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut player: Query<&mut Transform, With<Player>>,
) {
    let mut dir = Vec3::ZERO;
    for (key, d) in [
        (KeyCode::KeyW, Vec3::NEG_Z),
        (KeyCode::KeyS, Vec3::Z),
        (KeyCode::KeyA, Vec3::NEG_X),
        (KeyCode::KeyD, Vec3::X),
    ] {
        if keys.pressed(key) {
            dir += d;
        }
    }
    let mut transform = player.single_mut().unwrap();
    transform.translation += dir.normalize_or_zero() * 6.0 * time.delta_secs();
    transform.translation = transform
        .translation
        .clamp(Vec3::new(-9.5, 0.5, -9.5), Vec3::new(9.5, 0.5, 9.5));
}

fn spin_coin(time: Res<Time>, mut coins: Query<&mut Transform, With<Coin>>) {
    for mut transform in &mut coins {
        transform.rotate_y(2.0 * time.delta_secs());
    }
}

fn collect_coin(
    mut score: ResMut<Score>,
    player: Query<&Transform, With<Player>>,
    mut coins: Query<&mut Transform, (With<Coin>, Without<Player>)>,
) {
    let player = player.single().unwrap().translation;
    for mut coin in &mut coins {
        if coin.translation.distance(player) < 1.0 {
            score.0 += 1;
            coin.translation = coin_position(score.0);
        }
    }
}

/// Deterministic "random" spot so the example needs no RNG crate.
fn coin_position(n: u32) -> Vec3 {
    let angle = n as f32 * 2.4;
    let radius = 3.0 + (n % 4) as f32 * 1.5;
    Vec3::new(angle.cos() * radius, 0.5, angle.sin() * radius)
}

// ---------------------------------------------------------------------------
// gpui side: owns the window, forwards input, draws HUD and menus.
// ---------------------------------------------------------------------------

struct GameUi {
    bevy: EmbeddedBevy,
    viewport: ViewportImage,
    focus: FocusHandle,
    fps: f32,
    last_tick: Instant,
}

impl GameUi {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let bevy = EmbeddedBevy::new(UVec2::new(1280, 720), game_plugin);
        let focus = cx.focus_handle();
        focus.focus(window, cx);

        // Drive Bevy at ~60 Hz from gpui's executor.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    this.tick();
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
            focus,
            fps: 0.0,
            last_tick: Instant::now(),
        }
    }

    fn tick(&mut self) {
        self.bevy.update();
        let dt = self.last_tick.elapsed().as_secs_f32();
        self.last_tick = Instant::now();
        self.fps = self.fps * 0.9 + (1.0 / dt.max(1e-4)) * 0.1;
    }

    fn paused(&self) -> bool {
        self.bevy.world().resource::<Paused>().0
    }

    fn set_paused(&mut self, paused: bool) {
        self.bevy.world_mut().resource_mut::<Paused>().0 = paused;
    }

    fn reset(&mut self) {
        let world = self.bevy.world_mut();
        world.resource_mut::<Score>().0 = 0;
        let mut players = world.query_filtered::<&mut Transform, With<Player>>();
        for mut transform in players.iter_mut(world) {
            transform.translation = Vec3::new(0.0, 0.5, 0.0);
        }
        self.set_paused(false);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" && !event.is_held {
            let paused = self.paused();
            self.set_paused(!paused);
            cx.notify();
        } else if let Some(key) = key_code(&event.keystroke.key) {
            self.bevy
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(key);
        }
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if let Some(key) = key_code(&event.keystroke.key) {
            self.bevy
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .release(key);
        }
    }
}

/// Maps gpui key names to Bevy key codes (only the keys the game uses).
fn key_code(key: &str) -> Option<KeyCode> {
    Some(match key {
        "w" => KeyCode::KeyW,
        "a" => KeyCode::KeyA,
        "s" => KeyCode::KeyS,
        "d" => KeyCode::KeyD,
        _ => return None,
    })
}

impl Render for GameUi {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.viewport.sync(self.bevy.take_frame(), window);
        let score = self.bevy.world().resource::<Score>().0;
        let paused = self.paused();

        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .relative()
            .size_full()
            .bg(rgb(0x101010))
            .text_color(rgb(0xffffff))
            // 3D view rendered by Bevy.
            .children(self.viewport.current().map(|image| {
                img(image)
                    .absolute()
                    .size_full()
                    .object_fit(ObjectFit::Cover)
            }))
            // HUD.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_between()
                    .p_4()
                    .child(hud_badge(format!("Score: {score}")))
                    .child(hud_badge(format!("{:.0} FPS", self.fps))),
            )
            .child(
                div()
                    .absolute()
                    .bottom_4()
                    .left_4()
                    .child(hud_badge("WASD: move   Esc: menu")),
            )
            // Pause menu.
            .when(paused, |root| {
                root.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(rgba(0x000000aa))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .p_6()
                                .w(px(260.))
                                .rounded_lg()
                                .bg(rgb(0x2a2a2e))
                                .child(div().text_2xl().child("Paused"))
                                .child(menu_button("resume", "Resume").on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_paused(false);
                                        cx.notify();
                                    },
                                )))
                                .child(menu_button("reset", "Reset").on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.reset();
                                        cx.notify();
                                    },
                                ))),
                        ),
                )
            })
    }
}

fn hud_badge(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgba(0x00000099))
        .child(text.into())
}

fn menu_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_4()
        .py_2()
        .rounded_md()
        .bg(rgb(0x3d5afe))
        .hover(|style| style.bg(rgb(0x536dfe)))
        .cursor_pointer()
        .child(label)
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        let bounds = Bounds::centered(None, size(px(1280.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| GameUi::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
