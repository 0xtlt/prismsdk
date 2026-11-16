# 3D scenes

`scene3d()` draws meshes, glTF models, lights and a camera with the GPU on every backend: SceneKit (iOS, iPadOS, tvOS, visionOS, macOS, watchOS), WebGL 2 (web), OpenGL ES 3 (Android, Wear OS, Android TV) and OpenGL in a `GtkGLArea` (Linux). The scene is described in Rust like the rest of the UI; turning the camera, spinning, transitions and animation clips run on the device, without re-rendering the app.

```rust
use prism::prelude::*;
use prism::scene3d::*;

scene3d()
    .camera(Camera::perspective(40.).position(0., 3., 6.))   // looks at the origin
    .orbit(Orbit::new().auto_rotate(12.))                     // drag to turn, pinch or scroll to zoom
    .light(Light::ambient(rgb(0xffffff), 0.3))
    .light(Light::directional(rgb(0xffffff), 1.).direction(-0.6, -1., -0.4))
    .child(plane(6., 6.).color(rgb(0xe9e9ef)).roughness(0.9)) // the floor
    .child(cube(1.).name("cube").color(theme.accent).position(0., 0.5, 0.).spin(0., 40., 0.))
    .child(sphere(0.5).name("ball").metalness(1.).roughness(0.25).position(1.5, 0.5, 0.))
    .transition(Transition::spring(500))                     // changes animate
    .on_tap(cx.listener(|this, e: &SceneTapEvent, _| this.picked = e.node.clone()))
    .label("A cube and a metal ball on a floor")
    .h(px(300.))
```

- **Size**: a scene has no natural size: give it one (`.h(px(300.))`, `.flex_grow(1.)`). It does not shrink below it.
- **Coordinates**: right-handed, +Y up; the default camera stands at `(0, 2, 6)` and looks at the origin with a 45° vertical field of view. Angles are degrees; rotations apply Z first, then Y, then X (`R = Rx · Ry · Rz`). Units are yours (meters work well).
- **Background**: transparent by default, so the element's own `.bg(…)`, corners and borders show; `.background(color)` fills it.
- Colors are sRGB and may be adaptive (`Color::adaptive`): scenes follow light and dark mode live.

## Nodes

| Builder | Shape |
|---|---|
| `cube(size)`, `box3d(w, h, d)` | A box centered on its origin |
| `sphere(radius)` | A sphere |
| `plane(w, d)` | A rectangle lying flat (XZ), facing up |
| `cylinder(radius, height)`, `cone(radius, height)` | Standing along Y, centered |
| `torus(radius, tube)` | A ring lying flat |
| `mesh(Mesh::custom(&positions, Some(&indices)))` | Your triangles (counter-clockwise from the front); `.with_normals(…)`, `.with_uvs(…)`, `.with_colors(…)` |
| `group()` | Nothing drawn: moves, turns and scales its `.child(…)`ren together |

Every node takes `.position(x, y, z)`, `.rotation(x, y, z)` or `.quaternion(x, y, z, w)`, `.scale(s)` / `.scale_xyz(…)`, `.spin(x, y, z)` (degrees per second, continuous), `.hidden(bool)`, `.name(…)` (its identity across updates, reported by taps), `.label(…)` (a description that is not an identity: reported as the tap's `part`) and children placed relative to it.

**Materials** are physically based: `.color(c)` (its alpha makes the surface see-through), `.metalness(0…1)`, `.roughness(0…1)` (default 0.5), `.emissive(c)` (glows whatever the lights), `.unlit()` (flat color), `.double_sided()`, `.texture(src)` (an app asset, a URL or a `data:` URL, like `picture`; multiplied with the color), `.normal_map(src, scale)`, `.metal_rough_map(src)` (glTF's: G roughness, B metalness), `.occlusion_map(src)`, `.emissive_map(src)`, `.alpha_mask(cutoff)` (cut-out transparency: leaves, fences). `.material(Material { … })` sets everything at once.

## Models

```rust
static ROBOT: LazyLock<Model> =
    LazyLock::new(|| Model::from_glb(include_bytes!("robot.glb")).expect("a glTF 2.0 file"));

scene3d().child(
    ROBOT.node().name("robot")
        .play(ROBOT.clip("Walk").unwrap(), Playback::looping().fade(0.3)),
)
```

`Model::from_glb(bytes)` (or `Model::from_gltf(json, |uri| bytes)`) reads a glTF 2.0 file once: its node tree, meshes, materials and textures, skins and animations. `model.node()` is a group holding it (name it: two copies need two names); the file's node names become labels, so taps report the part hit. `model.clips()`, `model.clip("Walk")`, `model.bounds()`. Lights and cameras in the file are ignored.

## Instances

`cube(0.2).instances(&set)` draws the node's mesh once per instance in a single draw call. `Instances::new().push(position, rotation_deg, scale)` (`.push_quat(…)` with a quaternion and a scale per axis), `.color(c)` for the last one (multiplies the material), or `Instances::grid([nx, ny, nz], [dx, dy, dz])`. Taps report the `instance` hit. Build big sets once and keep them in your state.

## Lights and camera

- `Light::ambient(color, intensity)`, `Light::directional(color, intensity).direction(x, y, z)`, `Light::point(color, intensity).position(…).range(r)`, `Light::spot(color, intensity).position(…).direction(…).angle(deg)`. Up to 8 lights besides ambient ones; without any light (and no environment), a soft default rig (ambient and a key light from the upper left) lights the scene.
- **Shadows**: `.shadows(true)` on a directional light (the first one that asks). Every mesh casts and receives them; `.cast_shadows(false)`, `.receive_shadows(false)` opt a node out. The shadow map covers what is in view.
- **Environment**: `.environment(Environment::studio())` (also `::neutral()`, `::sunset()`, `::from_hdr(bytes)` for a Radiance `.hdr` image) lights everything with an image all around: metals reflect it, rough surfaces blur it. `.intensity(f)`, `.background(true)` to show it behind the scene.
- **Exposure**: colors are lit in linear space then tone mapped (ACES); `.exposure(f)` brightens (above 1) or darkens the picture.
- `Camera::perspective(fov).position(…).target(…)` or `Camera::orthographic(height)`; `.clip(near, far)`.
- `Orbit::new()` lets the user turn the camera around its target (drag: right turns the scene right, down shows more of its top), zoom (pinch, scroll wheel, trackpad; between a quarter and four times the starting distance, or `.distance(min, max)`) and, with `.pan(true)`, move the target (two fingers, right or Shift drag). `.auto_rotate(deg_per_s)` turns it by itself (paused while the user drags, off with Reduce Motion on the web); `Orbit::auto_only(deg_per_s)` without user control. Without `.orbit(…)`, the scene ignores drags, so scroll views around it scroll.

## Animation

- **Spin**: `.spin(0., 90., 0.)` turns a node a quarter turn per second around Y, on the device.
- **Transitions**: with `.transition(t)` on the scene, every change of a node's position, rotation, scale, color, emission, metalness or roughness, and of the camera, animates from where it is to its new value (nodes are matched by `name`, else by their place in the tree). A change in the middle of a transition starts from the current value. Euler rotations go through every degree in between, like 2D `rotate`; quaternions take the shorter way.
- **Clips**: `.play(clip, Playback::once())` (or `::looping()`, `.speed(f)`, `.fade(seconds)` to crossfade from the previous clip, `.restart(n)` to start over when `n` changes) plays keyframes on the device; `.stop()` or no `.play` stops. Clips come from models or are built: `AnimationClip::new("bob").translation("", &[0., 0.5, 1.], &[[0., 0., 0.], [0., 0.3, 0.], [0., 0., 0.]]).rotation("0/1", &times, &quats).scale("#lid", …).interpolation(Interp::CubicSpline)` — targets are `""` (the playing node), a child-index path from it (`"0/1"`: its first child's second child) or `"#name"`.
- Scenes draw only when something changes or moves (spin, auto-rotation, a transition, a clip, a gesture): a still scene costs no CPU.

## Taps

`.on_tap(handler)` is called with a `SceneTapEvent` when the scene is tapped or clicked (not dragged): `node` is the name of the nearest named node under the finger (a named `group()` names everything in it), `part` the nearest label from the mesh hit up to that node (a model's `"LeftArm"`), `instance` the instance hit, `point` where the ray hit, in scene coordinates; all `None` when it hit nothing. `e.is("cube")` compares. Skinned meshes are hit in their current pose.

## Performance

- **Updates** send only the nodes that changed; meshes, textures and clips travel once (a skinned 125 000-vertex character in a 28-node scene is 5.7 KB of JSON, the rest is cached binary).
- **Draws**: an instance set is one draw call; meshes outside the view are not drawn; draws are sorted to limit state changes; per-frame uniforms are set once per shader program.
- **Device work** per frame (turning the camera, playing clips, skinning matrices, culling 1 000 instances): well under a millisecond. Measured with a walking 125 000-vertex character, 1 000 instanced cubes, a shadow-casting light and an environment: 60 fps on the iOS simulator (10 draw calls); the GPU sets the rate elsewhere.
- Build big data once (`Model`, `Instances`, custom meshes) and keep it in your state or a `static`: building it in `render` hashes it on every render.
- Antialiasing: 4× MSAA everywhere (on Linux, not with software OpenGL, where it is too slow).

## Accessibility and limits

- A scene is decorative unless given `.label(…)`, read as an image.
- In widgets, Live Activities and car screens, a scene is not drawn (its box and background are).
- Alpha-masked surfaces (`alpha_mask`) cast solid shadows. On Apple platforms, a mesh both instanced and skinned is drawn in its bind pose, and SceneKit prefilters the environment itself (reflections can differ slightly from the other platforms).
- Not yet: USDZ models, glTF morph targets, cameras and lights; point and spot light shadows; per-node tap handlers.
- For games (a game loop, physics, input polling), see the Bevy integration planned next; `scene3d` is for 3D inside apps.

## How it works

The reconciler flattens each scene into a table of nodes keyed by name or place (`scene3d::wire`) and sends it whole the first time, then only the nodes that changed. Heavy data — meshes, instance sets, clips, skins, environments, images embedded in models — travels as content-addressed binary blobs (`scene3d::blobs`), fetched once by id (on the web from `/_prism/3d/<id>`, cached forever). `scene3d::player::ScenePlayer` is the reference implementation of what happens on the device (orbit, spin, transitions, clips and skins, culling, shadows, hit testing); the Linux backend uses it directly and the web (JavaScript), Android (Java) and Apple (Swift) backends port it, with the same triangles (`scene3d::geometry`) and, on OpenGL, the same shaders (`scene3d::shaders`), so every platform draws and moves the same scene the same way.
