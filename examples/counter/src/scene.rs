//! The 3D card of the Components tab: four shapes on a floor, lit by a
//! studio environment and a shadow-casting light. The camera turns by
//! itself and with the finger, a tapped shape lifts and glows, and
//! « Shuffle » swaps their places with a spring.

use prism::prelude::*;
use prism::scene3d::*;
use prism::ui::{self, Theme};

use crate::strings;

/// The shapes, by name, in their starting order.
const SHAPES: [&str; 4] = ["cube", "sphere", "cone", "ring"];
/// Where the shapes stand: the corners of a square around the center.
const SPOTS: [(f32, f32); 4] = [(-1.1, -1.1), (1.1, -1.1), (1.1, 1.1), (-1.1, 1.1)];

#[derive(Default)]
pub struct Scene {
    picked: Option<String>,
    /// How far the shapes moved around the square.
    turn: usize,
}

impl Scene {
    fn shape(&self, i: usize, t: &Theme) -> Node3d {
        let name = SHAPES[i];
        let (x, z) = SPOTS[(i + self.turn) % SPOTS.len()];
        let picked = self.picked.as_deref() == Some(name);
        let node = match name {
            "cube" => cube(0.9).color(t.accent).roughness(0.35).spin(0., 40., 0.),
            "sphere" => sphere(0.5)
                .color(rgb(0xD9D9E0))
                .metalness(1.)
                .roughness(0.25),
            "cone" => cone(0.5, 1.).color(rgb(0xF2A33A)).roughness(0.6),
            _ => torus(0.4, 0.14)
                .color(rgb(0x34C77B))
                .roughness(0.4)
                .rotation(70., 0., 0.)
                .spin(0., 0., 90.),
        };
        let base = if name == "ring" { 0.6 } else { 0.5 };
        node.name(name)
            .position(x, if picked { base + 0.4 } else { base }, z)
            .when(picked, |n| n.emissive(rgb(0x2A2A40)).scale(1.15))
    }
}

impl Render for Scene {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let picked = match &self.picked {
            Some(name) => strings::scene3d_picked(cx, display_name(cx, name)),
            None => strings::scene3d_nothing(cx),
        };
        let shapes: Vec<Node3d> = (0..SHAPES.len()).map(|i| self.shape(i, &t)).collect();
        ui::card()
            .child(ui::heading(strings::scene3d_title(cx)))
            .child(
                text(strings::scene3d_caption(cx))
                    .text_size(13.)
                    .text_color(t.muted),
            )
            .child(
                scene3d()
                    .camera(Camera::perspective(40.).position(0., 3.2, 6.2))
                    .orbit(Orbit::new().auto_rotate(12.))
                    // Studio light all around (metals reflect it), plus a key
                    // light casting shadows on the floor.
                    .environment(Environment::studio().intensity(0.8))
                    .light(
                        Light::directional(rgb(0xFFFFFF), 1.)
                            .direction(-0.6, -1., -0.4)
                            .shadows(true),
                    )
                    .light(
                        Light::point(rgb(0x8899FF), 0.5)
                            .position(0., 2.5, 2.5)
                            .range(8.),
                    )
                    .child(
                        plane(6., 6.)
                            .color(Color::adaptive(rgb(0xE9E9EF), rgb(0x2A2A31)))
                            .roughness(0.9),
                    )
                    .children(shapes)
                    .transition(Transition::spring(500))
                    .on_tap(cx.listener(|this, e: &SceneTapEvent, _| {
                        this.picked = e.node.clone().filter(|n| this.picked.as_ref() != Some(n));
                    }))
                    .label(strings::scene3d_label(cx))
                    .h(px(280.))
                    .rounded(12.)
                    .bg(Color::adaptive(rgb(0xF6F6F9), rgb(0x18181C))),
            )
            .child(
                div()
                    .flex_row()
                    .items_center()
                    .gap(12.)
                    .child(ui::button(strings::scene3d_shuffle(cx)).on_click(
                        cx.listener(|this, _, _| this.turn = (this.turn + 1) % SPOTS.len()),
                    ))
                    .child(text(picked).text_size(14.)),
            )
    }
}

/// A shape's name in the user's language.
fn display_name(cx: &App, name: &str) -> String {
    match name {
        "cube" => strings::scene3d_names_cube(cx),
        "sphere" => strings::scene3d_names_sphere(cx),
        "cone" => strings::scene3d_names_cone(cx),
        _ => strings::scene3d_names_ring(cx),
    }
}
