// 3D scenes (`scene3d()`, prism-core's `scene3d` module) drawn with SceneKit
// on every Apple platform.
//
// `PrismScenePlayer` ports prism-core's `player.rs` line by line (the
// retained node table patched by `wire.rs` updates, orbit camera, spins,
// transitions, clips with crossfades, skins, culling, the shadow map's
// fitting, hit testing), with the blob decoders of `blobs.rs` and `env.rs`,
// `clip.rs`'s sampler and `geometry.rs`'s primitives, so scenes move,
// animate and pick exactly like on the other backends. SceneKit only draws:
//
// - every visible mesh is a flat child of the root at the world matrix the
//   player computed; a skinned one is placed by bone nodes set to the
//   player's `world(joint) · inverse_bind` matrices (SceneKit skins on the
//   GPU, like the `SKINNED` shader); an instanced one is a single merged
//   geometry built once per mesh and instance set (one draw call);
// - materials are `.physicallyBased` with the full texture set; a surface
//   shader modifier multiplies the factors in (color × texture × vertex
//   color, metal-rough channels, emission, alpha mask/opaque) and a
//   fragment modifier applies `shaders.rs`'s exposure, ACES (Narkowicz) and
//   gamma, so the look matches the OpenGL backends (SceneKit's own HDR
//   pipeline stays off);
// - the environment's sharpest level is SceneKit's `lightingEnvironment`
//   (SceneKit prefilters it) and, tone mapped, the background;
// - the first directional light with `shadows` casts SceneKit's forward
//   shadow, its orthographic projection fitted like the reference's
//   `ShadowPass`.
//
// UIKit (iOS, iPadOS, tvOS, visionOS) and AppKit: `PrismSceneView`, created
// by Rust through `prism_scene_create`. watchOS: PrismWatch.swift hosts a
// `PrismSceneRenderer` in SwiftUI's `SceneView`. Blobs come from Rust,
// synchronously, through `prism_scene_blob` (prism-native).
import Foundation
import SceneKit
import simd
#if os(macOS)
import AppKit
typealias PrismSceneColor = NSColor
typealias PrismSceneImage = NSImage
#else
import UIKit
typealias PrismSceneColor = UIColor
typealias PrismSceneImage = UIImage
#endif

typealias PrismVec3 = SIMD3<Float>
/// A unit quaternion `[x, y, z, w]`.
typealias PrismQuat = SIMD4<Float>
typealias PrismMat4 = simd_float4x4

// MARK: Wire format (prism-core `scene3d::wire`: serde names, defaults when absent)

private func sceneNum(_ v: Any?, _ d: Float) -> Float { (v as? NSNumber)?.floatValue ?? d }
private func sceneBool(_ v: Any?, _ d: Bool) -> Bool { (v as? NSNumber)?.boolValue ?? d }
private func sceneFloats(_ v: Any?) -> [Float] { (v as? [NSNumber])?.map(\.floatValue) ?? [] }
private func sceneVec3(_ v: Any?, _ d: PrismVec3) -> PrismVec3 {
    let a = sceneFloats(v)
    return a.count >= 3 ? PrismVec3(a[0], a[1], a[2]) : d
}

/// `[r, g, b, a]` or `[r, g, b, a, dr, dg, db, da]` (dark variant), sRGB 0 to 1.
struct PrismSceneRgba: Equatable {
    var light: SIMD4<Float>
    var dark: SIMD4<Float>?

    static let black = PrismSceneRgba(light: [0, 0, 0, 1], dark: nil)
    static let white = PrismSceneRgba(light: [1, 1, 1, 1], dark: nil)

    init(light: SIMD4<Float>, dark: SIMD4<Float>?) {
        self.light = light
        self.dark = dark
    }

    init?(_ v: Any?) {
        guard let a = (v as? [NSNumber])?.map(\.floatValue) else { return nil }
        func at(_ i: Int, _ d: Float) -> Float { i < a.count ? a[i] : d }
        light = [at(0, 0), at(1, 0), at(2, 0), at(3, 1)]
        dark = a.count >= 8 ? [at(4, 0), at(5, 0), at(6, 0), at(7, 1)] : nil
    }

    func resolve(_ dark: Bool) -> SIMD4<Float> { dark ? (self.dark ?? light) : light }

    func lerp(_ to: PrismSceneRgba, _ t: Float) -> PrismSceneRgba {
        let m = { (x: SIMD4<Float>, y: SIMD4<Float>) in x + (y - x) * t }
        return PrismSceneRgba(light: m(light, to.light),
                              dark: dark != nil || to.dark != nil ? m(resolve(true), to.resolve(true)) : nil)
    }
}

/// An axis-aligned box (`math::Aabb`: min xyz, max xyz).
struct PrismAabb: Equatable {
    var lo: PrismVec3
    var hi: PrismVec3

    static let zero = PrismAabb(lo: .zero, hi: .zero)

    init(lo: PrismVec3, hi: PrismVec3) {
        self.lo = lo
        self.hi = hi
    }

    init(_ v: Any?) {
        let a = sceneFloats(v)
        self = a.count >= 6 ? PrismAabb(lo: [a[0], a[1], a[2]], hi: [a[3], a[4], a[5]]) : .zero
    }

    init(floats a: [Float]) {
        self = a.count >= 6 ? PrismAabb(lo: [a[0], a[1], a[2]], hi: [a[3], a[4], a[5]]) : .zero
    }

    var center: PrismVec3 { (lo + hi) / 2 }

    /// `aabb_union`.
    func union(_ b: PrismAabb) -> PrismAabb {
        PrismAabb(lo: PrismVec3(min(lo.x, b.lo.x), min(lo.y, b.lo.y), min(lo.z, b.lo.z)),
                  hi: PrismVec3(max(hi.x, b.hi.x), max(hi.y, b.hi.y), max(hi.z, b.hi.z)))
    }

    /// `transform_aabb`: the box holding this one once transformed by `m`.
    func transformed(_ m: PrismMat4) -> PrismAabb {
        let half = (hi - lo) / 2
        let c = PrismSceneMath.transformPoint(m, center)
        let c0 = m.columns.0, c1 = m.columns.1, c2 = m.columns.2
        let e = PrismVec3(abs(c0.x) * half.x + abs(c1.x) * half.y + abs(c2.x) * half.z,
                          abs(c0.y) * half.x + abs(c1.y) * half.y + abs(c2.y) * half.z,
                          abs(c0.z) * half.x + abs(c1.z) * half.y + abs(c2.z) * half.z)
        return PrismAabb(lo: c - e, hi: c + e)
    }
}

enum PrismSceneMesh: Equatable {
    case box(PrismVec3)
    case sphere(Float)
    case plane(SIMD2<Float>)
    case cylinder(Float, Float)
    case cone(Float, Float)
    case torus(Float, Float)
    /// An `M` blob and its positions' box.
    case blob(id: String, bounds: PrismAabb)

    init?(_ v: Any?) {
        guard let d = v as? [String: Any] else { return nil }
        switch d["type"] as? String {
        case "box": self = .box(sceneVec3(d["size"], [1, 1, 1]))
        case "sphere": self = .sphere(sceneNum(d["radius"], 0.5))
        case "plane":
            let s = sceneFloats(d["size"])
            self = .plane(s.count >= 2 ? SIMD2(s[0], s[1]) : SIMD2(1, 1))
        case "cylinder": self = .cylinder(sceneNum(d["radius"], 0.5), sceneNum(d["height"], 1))
        case "cone": self = .cone(sceneNum(d["radius"], 0.5), sceneNum(d["height"], 1))
        case "torus": self = .torus(sceneNum(d["radius"], 1), sceneNum(d["tube"], 0.25))
        case "blob":
            guard let id = d["id"] as? String else { return nil }
            self = .blob(id: id, bounds: PrismAabb(d["bounds"]))
        default: return nil
        }
    }

    /// `Mesh::bounds`: its box in its own space.
    var bounds: PrismAabb {
        func b(_ x: Float, _ y: Float, _ z: Float) -> PrismAabb { PrismAabb(lo: [-x, -y, -z], hi: [x, y, z]) }
        switch self {
        case let .box(s): return b(s.x / 2, s.y / 2, s.z / 2)
        case let .sphere(r): return b(r, r, r)
        case let .plane(s): return b(s.x / 2, 0, s.y / 2)
        case let .cylinder(r, h), let .cone(r, h): return b(r, h / 2, r)
        case let .torus(r, t): return b(r + t, t, r + t)
        case let .blob(_, bounds): return bounds
        }
    }

    /// `Mesh::key`: equal keys, equal triangles (geometry is cached by it).
    var key: String {
        switch self {
        case let .box(s): return "box:\(s.x),\(s.y),\(s.z)"
        case let .sphere(r): return "sphere:\(r)"
        case let .plane(s): return "plane:\(s.x),\(s.y)"
        case let .cylinder(r, h): return "cylinder:\(r),\(h)"
        case let .cone(r, h): return "cone:\(r),\(h)"
        case let .torus(r, t): return "torus:\(r),\(t)"
        case let .blob(id, _): return id
        }
    }
}

/// How a material uses its alpha.
enum PrismSceneAlpha: Equatable {
    case blend, mask, opaque
}

struct PrismSceneMaterial: Equatable {
    var color = PrismSceneRgba(light: [0.8, 0.8, 0.8, 1], dark: nil)
    var metalness: Float = 0
    var roughness: Float = 0.5
    var emissive: PrismSceneRgba?
    var unlit = false
    var doubleSided = false
    /// Base-color image (sRGB).
    var texture: String?
    /// glTF metallic-roughness image: G roughness, B metalness.
    var mrMap: String?
    var normalMap: String?
    var normalScale: Float = 1
    var emissiveMap: String?
    /// Ambient occlusion (R).
    var aoMap: String?
    var alpha = PrismSceneAlpha.blend
    var cutoff: Float = 0
    var vcolor = false

    init() {}

    init(_ v: Any?) {
        guard let d = v as? [String: Any] else { return }
        color = PrismSceneRgba(d["color"]) ?? color
        metalness = sceneNum(d["metal"], 0)
        roughness = sceneNum(d["rough"], 0.5)
        emissive = PrismSceneRgba(d["emissive"])
        unlit = sceneBool(d["unlit"], false)
        doubleSided = sceneBool(d["double"], false)
        texture = d["map"] as? String
        mrMap = d["mr_map"] as? String
        normalMap = d["normal_map"] as? String
        normalScale = sceneNum(d["normal_scale"], 1)
        emissiveMap = d["emissive_map"] as? String
        aoMap = d["ao_map"] as? String
        switch d["alpha"] as? String {
        case "mask": alpha = .mask
        case "opaque": alpha = .opaque
        default: alpha = .blend
        }
        cutoff = sceneNum(d["cutoff"], 0)
        vcolor = sceneBool(d["vcolor"], false)
    }

    /// Every image source it uses.
    var textures: [String] { [texture, mrMap, normalMap, emissiveMap, aoMap].compactMap { $0 } }
}

struct PrismSceneCamera: Equatable {
    var position: PrismVec3 = [0, 2, 6]
    var target: PrismVec3 = .zero
    var fov: Float = 45
    var near: Float = 0.1
    var far: Float = 1000
    var ortho: Float?

    init() {}

    init(_ v: Any?) {
        guard let d = v as? [String: Any] else { return }
        position = sceneVec3(d["pos"], position)
        target = sceneVec3(d["target"], target)
        fov = sceneNum(d["fov"], fov)
        near = sceneNum(d["near"], near)
        far = sceneNum(d["far"], far)
        ortho = (d["ortho"] as? NSNumber)?.floatValue
    }
}

struct PrismSceneOrbit: Equatable {
    var rotate = true
    var zoom = true
    var pan = false
    /// Auto-rotation, degrees per second.
    var auto: Float = 0
    var min: Float = 0
    var max: Float = 0

    init() {}

    init(_ d: [String: Any]) {
        rotate = sceneBool(d["rotate"], true)
        zoom = sceneBool(d["zoom"], true)
        pan = sceneBool(d["pan"], false)
        auto = sceneNum(d["auto"], 0)
        min = sceneNum(d["min"], 0)
        max = sceneNum(d["max"], 0)
    }
}

struct PrismSceneLight: Equatable {
    enum Kind { case ambient, directional, point, spot }
    var kind: Kind
    var color: PrismSceneRgba
    var intensity: Float
    var position: PrismVec3 = .zero
    var direction: PrismVec3 = .zero
    var angle: Float = 0
    var range: Float = 0
    var shadows = false

    init(kind: Kind, color: PrismSceneRgba, intensity: Float, direction: PrismVec3 = .zero) {
        self.kind = kind
        self.color = color
        self.intensity = intensity
        self.direction = direction
    }

    init?(_ d: [String: Any]) {
        switch d["type"] as? String {
        case "ambient": kind = .ambient
        case "directional": kind = .directional
        case "point": kind = .point
        case "spot": kind = .spot
        default: return nil
        }
        color = PrismSceneRgba(d["color"]) ?? .white
        intensity = sceneNum(d["intensity"], 1)
        position = sceneVec3(d["pos"], .zero)
        direction = sceneVec3(d["dir"], .zero)
        angle = sceneNum(d["angle"], 0)
        range = sceneNum(d["range"], 0)
        shadows = sceneBool(d["shadows"], false)
    }
}

struct PrismSceneTransition: Equatable {
    var ms: Float
    var delay: Float
    var bezier: SIMD4<Float>
}

/// `EnvSettings`: `{"id", "intensity", "background"}`.
struct PrismSceneEnvSettings: Equatable {
    var id: String
    var intensity: Float
    var background: Bool
}

/// `SceneSettings`: everything about a scene but its nodes.
struct PrismSceneSettings: Equatable {
    var background: PrismSceneRgba?
    var camera = PrismSceneCamera()
    var orbit: PrismSceneOrbit?
    var lights: [PrismSceneLight] = []
    var transition: PrismSceneTransition?
    var tap = false
    var env: PrismSceneEnvSettings?
    var exposure: Float = 1

    init() {}

    init(_ d: [String: Any]) {
        background = PrismSceneRgba(d["bg"])
        camera = PrismSceneCamera(d["camera"])
        orbit = (d["orbit"] as? [String: Any]).map(PrismSceneOrbit.init)
        lights = (d["lights"] as? [[String: Any]] ?? []).compactMap(PrismSceneLight.init)
        if let t = d["t"] as? [String: Any] {
            let b = sceneFloats(t["bezier"])
            transition = PrismSceneTransition(ms: sceneNum(t["ms"], 0), delay: sceneNum(t["delay"], 0),
                                              bezier: b.count >= 4 ? SIMD4(b[0], b[1], b[2], b[3]) : SIMD4(0.25, 0.1, 0.25, 1))
        }
        tap = sceneBool(d["tap"], false)
        if let e = d["env"] as? [String: Any], let id = e["id"] as? String {
            env = PrismSceneEnvSettings(id: id, intensity: sceneNum(e["intensity"], 1),
                                        background: sceneBool(e["background"], false))
        }
        exposure = sceneNum(d["exposure"], 1)
    }

    /// The lights, or — with neither lights nor an environment — the default
    /// rig (`SceneSettings::effective_lights`).
    var effectiveLights: [PrismSceneLight] {
        if !lights.isEmpty || env != nil { return lights }
        return [PrismSceneLight(kind: .ambient, color: .white, intensity: 0.35),
                PrismSceneLight(kind: .directional, color: .white, intensity: 0.9, direction: [-0.5, -1, -0.6])]
    }
}

/// `InstancesRef`: `{"id", "count", "bounds"}`.
struct PrismSceneInstancesRef: Equatable {
    var id: String
    var count: Int
    var bounds: PrismAabb
}

/// `PlayRef`: `{"clip", "speed", "loop", "fade", "run"}`.
struct PrismScenePlayRef: Equatable {
    var clip: String
    var speed: Float
    var looping: Bool
    var fade: Float
    var run: UInt32
}

/// `SkinRef`: `{"id", "root"}`.
struct PrismSceneSkinRef: Equatable {
    var id: String
    var root: String
}

/// `FlatNode`: one node without its children.
struct PrismSceneFlatNode: Equatable {
    var key: String
    var parent: String?
    var name: String?
    var label: String?
    var mesh: PrismSceneMesh?
    var material: PrismSceneMaterial
    var position: PrismVec3
    var rotation: PrismVec3
    var quaternion: PrismQuat?
    var scale: PrismVec3
    var spin: PrismVec3
    var hidden: Bool
    var instances: PrismSceneInstancesRef?
    var play: PrismScenePlayRef?
    var skin: PrismSceneSkinRef?
    var castShadows: Bool
    var receiveShadows: Bool

    init(_ d: [String: Any]) {
        key = d["key"] as? String ?? ""
        parent = d["parent"] as? String
        name = d["name"] as? String
        label = d["label"] as? String
        mesh = PrismSceneMesh(d["mesh"])
        material = PrismSceneMaterial(d["mat"])
        position = sceneVec3(d["pos"], .zero)
        rotation = sceneVec3(d["rot"], .zero)
        let q = sceneFloats(d["quat"])
        quaternion = q.count >= 4 ? PrismQuat(q[0], q[1], q[2], q[3]) : nil
        scale = sceneVec3(d["scale"], [1, 1, 1])
        spin = sceneVec3(d["spin"], .zero)
        hidden = sceneBool(d["hidden"], false)
        if let i = d["inst"] as? [String: Any], let id = i["id"] as? String {
            instances = PrismSceneInstancesRef(id: id, count: (i["count"] as? NSNumber)?.intValue ?? 0,
                                               bounds: PrismAabb(i["bounds"]))
        }
        if let p = d["play"] as? [String: Any], let clip = p["clip"] as? String {
            play = PrismScenePlayRef(clip: clip, speed: sceneNum(p["speed"], 1), looping: sceneBool(p["loop"], false),
                                     fade: sceneNum(p["fade"], 0), run: (p["run"] as? NSNumber)?.uint32Value ?? 0)
        }
        if let s = d["skin"] as? [String: Any], let id = s["id"] as? String {
            skin = PrismSceneSkinRef(id: id, root: s["root"] as? String ?? "")
        }
        castShadows = sceneBool(d["cast"], true)
        receiveShadows = sceneBool(d["recv"], true)
    }
}

/// `SceneUpdate`: the whole scene, or the changes since the last update.
enum PrismSceneUpdate {
    case full(PrismSceneSettings, [PrismSceneFlatNode])
    case diff(settings: PrismSceneSettings?, upsert: [PrismSceneFlatNode], remove: [String], order: [String]?)

    /// Reads an update's JSON object (`nil` if it is not version 2).
    init?(_ d: [String: Any]) {
        guard (d["v"] as? NSNumber)?.intValue == 2 else { return nil }
        let nodes = { (k: String) in (d[k] as? [[String: Any]] ?? []).map(PrismSceneFlatNode.init) }
        let settings = (d["settings"] as? [String: Any]).map(PrismSceneSettings.init)
        if sceneBool(d["full"], false) {
            self = .full(settings ?? PrismSceneSettings(), nodes("nodes"))
        } else {
            self = .diff(settings: settings, upsert: nodes("upsert"), remove: d["remove"] as? [String] ?? [],
                         order: d["order"] as? [String])
        }
    }

    init?(json: Data) {
        guard let d = (try? JSONSerialization.jsonObject(with: json)) as? [String: Any] else { return nil }
        self.init(d)
    }

    /// The nodes it carries: all of them (full) or the upserted ones.
    var nodes: [PrismSceneFlatNode] {
        switch self {
        case let .full(_, n): return n
        case let .diff(_, n, _, _): return n
        }
    }

    /// The settings it carries (always for a full update).
    var settings: PrismSceneSettings? {
        switch self {
        case let .full(s, _): return s
        case let .diff(s, _, _, _): return s
        }
    }

    /// Every texture source of its nodes.
    var textures: [String] { nodes.flatMap(\.material.textures) }
}

// MARK: Math (prism-core `scene3d::math`: column-major, right-handed, OpenGL clip space)

enum PrismSceneMath {
    static func rad(_ deg: Float) -> Float { deg * .pi / 180 }
    static func deg(_ rad: Float) -> Float { rad * 180 / .pi }

    static func translation(_ v: PrismVec3) -> PrismMat4 {
        PrismMat4(columns: ([1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [v.x, v.y, v.z, 1]))
    }

    static func scaling(_ v: PrismVec3) -> PrismMat4 { PrismMat4(diagonal: [v.x, v.y, v.z, 1]) }

    static func rotationX(_ d: Float) -> PrismMat4 {
        let (s, c) = (sin(rad(d)), cos(rad(d)))
        return PrismMat4(columns: ([1, 0, 0, 0], [0, c, s, 0], [0, -s, c, 0], [0, 0, 0, 1]))
    }

    static func rotationY(_ d: Float) -> PrismMat4 {
        let (s, c) = (sin(rad(d)), cos(rad(d)))
        return PrismMat4(columns: ([c, 0, -s, 0], [0, 1, 0, 0], [s, 0, c, 0], [0, 0, 0, 1]))
    }

    static func rotationZ(_ d: Float) -> PrismMat4 {
        let (s, c) = (sin(rad(d)), cos(rad(d)))
        return PrismMat4(columns: ([c, s, 0, 0], [-s, c, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]))
    }

    /// Euler angles in degrees: `Rx · Ry · Rz` (Z applies first).
    static func rotation(_ d: PrismVec3) -> PrismMat4 { rotationX(d.x) * rotationY(d.y) * rotationZ(d.z) }

    /// A node's local matrix: `T · R · S`.
    static func compose(_ p: PrismVec3, _ r: PrismVec3, _ s: PrismVec3) -> PrismMat4 {
        translation(p) * rotation(r) * scaling(s)
    }

    /// `T · R(q) · S`.
    static func composeQuat(_ p: PrismVec3, _ q: PrismQuat, _ s: PrismVec3) -> PrismMat4 {
        var m = quatToMat4(q)
        m.columns.0 *= s.x
        m.columns.1 *= s.y
        m.columns.2 *= s.z
        m.columns.3 = SIMD4(p.x, p.y, p.z, 1)
        return m
    }

    static func quatToMat4(_ q: PrismQuat) -> PrismMat4 {
        let (x, y, z, w) = (q.x, q.y, q.z, q.w)
        let (xx, yy, zz) = (x * x, y * y, z * z)
        let (xy, xz, yz) = (x * y, x * z, y * z)
        let (wx, wy, wz) = (w * x, w * y, w * z)
        return PrismMat4(columns: ([1 - 2 * (yy + zz), 2 * (xy + wz), 2 * (xz - wy), 0],
                                   [2 * (xy - wz), 1 - 2 * (xx + zz), 2 * (yz + wx), 0],
                                   [2 * (xz + wy), 2 * (yz - wx), 1 - 2 * (xx + yy), 0],
                                   [0, 0, 0, 1]))
    }

    /// `a · b` (`b` applies first).
    static func quatMul(_ a: PrismQuat, _ b: PrismQuat) -> PrismQuat {
        PrismQuat(a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
                  a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
                  a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
                  a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z)
    }

    /// The quaternion of Euler angles in degrees (`Rx · Ry · Rz`).
    static func quatFromEuler(_ d: PrismVec3) -> PrismQuat {
        func half(_ a: Float) -> (Float, Float) { (sin(rad(a) / 2), cos(rad(a) / 2)) }
        let (sx, cx) = half(d.x), (sy, cy) = half(d.y), (sz, cz) = half(d.z)
        return quatMul([sx, 0, 0, cx], quatMul([0, sy, 0, cy], [0, 0, sz, cz]))
    }

    /// Unit length (the identity if zero or not finite).
    static func quatNormalize(_ q: PrismQuat) -> PrismQuat {
        let l = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).squareRoot()
        return l > 1e-12 && l.isFinite ? q / l : [0, 0, 0, 1]
    }

    /// The shorter way around (normalized lerp when nearly equal).
    static func slerp(_ a: PrismQuat, _ b: PrismQuat, _ t: Float) -> PrismQuat {
        var d = simd_dot(a, b)
        var b = b
        if d < 0 {
            d = -d
            b = -b
        }
        if d > 0.9995 { return quatNormalize(a + (b - a) * t) }
        let theta = acos(min(max(d, -1), 1))
        let s = sin(theta)
        let (wa, wb) = (sin((1 - t) * theta) / s, sin(t * theta) / s)
        return quatNormalize(a * wa + b * wb)
    }

    static func perspective(_ fov: Float, _ aspect: Float, _ near: Float, _ far: Float) -> PrismMat4 {
        let f = 1 / tan(rad(fov) / 2), nf = 1 / (near - far)
        return PrismMat4(columns: ([f / aspect, 0, 0, 0], [0, f, 0, 0], [0, 0, (far + near) * nf, -1],
                                   [0, 0, 2 * far * near * nf, 0]))
    }

    static func orthographic(_ height: Float, _ aspect: Float, _ near: Float, _ far: Float) -> PrismMat4 {
        let t = height / 2, r = height / 2 * aspect
        return PrismMat4(columns: ([1 / r, 0, 0, 0], [0, 1 / t, 0, 0], [0, 0, -2 / (far - near), 0],
                                   [0, 0, -(far + near) / (far - near), 1]))
    }

    static func normalize(_ v: PrismVec3) -> PrismVec3 {
        let l = simd_length(v)
        return l == 0 ? v : v / l
    }

    static func lookAt(_ eye: PrismVec3, _ target: PrismVec3, _ up: PrismVec3) -> PrismMat4 {
        var f = normalize(target - eye)
        if simd_length(f) == 0 { f = [0, 0, -1] }
        var s = simd_cross(f, up)
        if simd_length(s) < 1e-6 { s = simd_cross(f, [0, 0, -1]) } // looking straight up or down
        s = normalize(s)
        let u = simd_cross(s, f)
        return PrismMat4(columns: ([s.x, u.x, -f.x, 0], [s.y, u.y, -f.y, 0], [s.z, u.z, -f.z, 0],
                                   [-simd_dot(s, eye), -simd_dot(u, eye), simd_dot(f, eye), 1]))
    }

    /// The inverse (the identity if `m` is singular: a zero determinant;
    /// a centimeter rig's tiny one is fine).
    static func invert(_ m: PrismMat4) -> PrismMat4 {
        let det = m.determinant
        if det == 0 || !det.isFinite { return matrix_identity_float4x4 }
        let r = m.inverse
        let c = [r.columns.0, r.columns.1, r.columns.2, r.columns.3]
        return c.allSatisfy { $0.x.isFinite && $0.y.isFinite && $0.z.isFinite && $0.w.isFinite } ? r : matrix_identity_float4x4
    }

    static func transformPoint(_ m: PrismMat4, _ p: PrismVec3) -> PrismVec3 {
        let v = m * SIMD4(p.x, p.y, p.z, 1)
        return v.w != 0 && v.w != 1 ? PrismVec3(v.x, v.y, v.z) / v.w : PrismVec3(v.x, v.y, v.z)
    }

    static func transformDir(_ m: PrismMat4, _ v: PrismVec3) -> PrismVec3 {
        let r = m * SIMD4(v.x, v.y, v.z, 0)
        return PrismVec3(r.x, r.y, r.z)
    }

    /// The six clip planes of `vp` (left, right, bottom, top, near, far).
    static func frustumPlanes(_ vp: PrismMat4) -> [SIMD4<Float>] {
        let t = vp.transpose
        let (r0, r1, r2, r3) = (t.columns.0, t.columns.1, t.columns.2, t.columns.3)
        return [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 + r2, r3 - r2]
    }

    /// At least partly inside the frustum (conservative).
    static func aabbInFrustum(_ planes: [SIMD4<Float>], _ b: PrismAabb) -> Bool {
        planes.allSatisfy { p in
            let x = p.x >= 0 ? b.hi.x : b.lo.x
            let y = p.y >= 0 ? b.hi.y : b.lo.y
            let z = p.z >= 0 ? b.hi.z : b.lo.z
            return p.x * x + p.y * y + p.z * z + p.w >= 0
        }
    }

    /// Where the ray `origin + t · dir` enters `b` (the slab test).
    static func rayAabb(_ origin: PrismVec3, _ dir: PrismVec3, _ b: PrismAabb) -> Float? {
        var (t0, t1): (Float, Float) = (0, .greatestFiniteMagnitude)
        for a in 0..<3 {
            let (lo, hi) = (b.lo[a], b.hi[a])
            if abs(dir[a]) < 1e-12 {
                if origin[a] < lo || origin[a] > hi { return nil }
                continue
            }
            let inv = 1 / dir[a]
            var (n, f) = ((lo - origin[a]) * inv, (hi - origin[a]) * inv)
            if n > f { swap(&n, &f) }
            t0 = max(t0, n)
            t1 = min(t1, f)
            if t0 > t1 { return nil }
        }
        return t0
    }

    static func lerp(_ a: Float, _ b: Float, _ t: Float) -> Float { a + (b - a) * t }

    static func srgbToLinear(_ c: Float) -> Float { c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4) }
}

// MARK: Geometry (prism-core `scene3d::geometry`)

/// Segments around a sphere, cylinder, cone or torus.
private let sceneSegments: UInt32 = 48
/// Rings from pole to pole of a sphere, and around a torus' tube.
private let sceneRings: UInt32 = 24

/// Triangles of a mesh (`MeshData`): counter-clockwise seen from outside,
/// normals out, texture coordinates `(0, 0)` at the image's top-left.
/// Optional attributes (colors, skin) are empty when absent.
struct PrismSceneMeshData {
    var positions: [Float] = []
    var normals: [Float] = []
    var uvs: [Float] = []
    /// Linear RGBA per vertex.
    var colors: [Float] = []
    /// Four joint indices per vertex.
    var joints: [UInt16] = []
    var weights: [Float] = []
    var indices: [UInt32] = []

    var vertexCount: Int { positions.count / 3 }

    private mutating func vertex(_ p: PrismVec3, _ n: PrismVec3, _ u: Float, _ v: Float) -> UInt32 {
        let i = UInt32(positions.count / 3)
        positions += [p.x, p.y, p.z]
        normals += [n.x, n.y, n.z]
        uvs += [u, v]
        return i
    }

    private mutating func triangle(_ a: UInt32, _ b: UInt32, _ c: UInt32) { indices += [a, b, c] }

    func position(_ i: UInt32) -> PrismVec3 {
        let i = Int(i) * 3
        return PrismVec3(positions[i], positions[i + 1], positions[i + 2])
    }

    init() {}

    /// A primitive's triangles (blob meshes come from `PrismSceneBlobs`).
    init(primitive mesh: PrismSceneMesh) {
        switch mesh {
        case let .box(size): self = Self.cuboid(size)
        case let .sphere(r): self = Self.sphere(r)
        case let .plane(size): self = Self.plane(size)
        case let .cylinder(r, h): self = Self.cylinder(r, r, h)
        case let .cone(r, h): self = Self.cylinder(r, 0, h)
        case let .torus(r, t): self = Self.torus(r, t)
        case .blob: self = Self()
        }
    }

    static func cuboid(_ size: PrismVec3) -> Self {
        let half = size / 2
        var m = Self()
        // (normal, right, up) per face, seen from outside: right × up = normal.
        let faces: [(PrismVec3, PrismVec3, PrismVec3)] = [
            ([1, 0, 0], [0, 0, -1], [0, 1, 0]),
            ([-1, 0, 0], [0, 0, 1], [0, 1, 0]),
            ([0, 1, 0], [1, 0, 0], [0, 0, -1]),
            ([0, -1, 0], [1, 0, 0], [0, 0, 1]),
            ([0, 0, 1], [1, 0, 0], [0, 1, 0]),
            ([0, 0, -1], [-1, 0, 0], [0, 1, 0]),
        ]
        func extent(_ v: PrismVec3) -> Float { simd_dot(simd_abs(v), half) }
        for (n, r, u) in faces {
            let (dn, dr, du) = (extent(n), extent(r), extent(u))
            func at(_ sr: Float, _ su: Float) -> PrismVec3 { n * dn + r * (dr * sr) + u * (du * su) }
            // Top-left, top-right, bottom-right, bottom-left.
            let a = m.vertex(at(-1, 1), n, 0, 0)
            let b = m.vertex(at(1, 1), n, 1, 0)
            let c = m.vertex(at(1, -1), n, 1, 1)
            let d = m.vertex(at(-1, -1), n, 0, 1)
            m.triangle(a, d, c)
            m.triangle(a, c, b)
        }
        return m
    }

    static func sphere(_ radius: Float) -> Self {
        var m = Self()
        let (rings, segs) = (sceneRings, sceneSegments)
        for i in 0...rings {
            let v = Float(i) / Float(rings)
            let theta = v * .pi
            for j in 0...segs {
                let u = Float(j) / Float(segs)
                let phi = u * 2 * .pi
                let n = PrismVec3(-sin(theta) * sin(phi), cos(theta), sin(theta) * cos(phi))
                _ = m.vertex(n * radius, n, u, v)
            }
        }
        let row = segs + 1
        for i in 0..<rings {
            for j in 0..<segs {
                let (a, b) = (i * row + j, i * row + j + 1)
                let (c, d) = ((i + 1) * row + j, (i + 1) * row + j + 1)
                if i != 0 { m.triangle(a, b, c) }
                if i != rings - 1 { m.triangle(b, d, c) }
            }
        }
        return m
    }

    /// In the XZ plane, facing +Y; u along +X, v along +Z.
    static func plane(_ size: SIMD2<Float>) -> Self {
        let (hx, hz) = (size.x / 2, size.y / 2)
        var m = Self()
        let n = PrismVec3(0, 1, 0)
        let a = m.vertex([-hx, 0, -hz], n, 0, 0)
        let b = m.vertex([hx, 0, -hz], n, 1, 0)
        let c = m.vertex([hx, 0, hz], n, 1, 1)
        let d = m.vertex([-hx, 0, hz], n, 0, 1)
        m.triangle(a, d, c)
        m.triangle(a, c, b)
        return m
    }

    /// Along Y, centered (a cone when `top` is 0): side, top cap, bottom cap.
    static func cylinder(_ bottom: Float, _ top: Float, _ height: Float) -> Self {
        var m = Self()
        let segs = sceneSegments
        let h = height / 2
        // The side's normal leans up by the slope.
        let slope = height > 0 ? (bottom - top) / height : 0
        func around(_ j: UInt32) -> (Float, Float, Float) {
            let u = Float(j) / Float(segs)
            let phi = u * 2 * .pi
            return (u, sin(phi), cos(phi))
        }
        for j in 0...segs {
            let (u, s, c) = around(j)
            let n = PrismSceneMath.normalize([s, slope, c])
            _ = m.vertex([top * s, h, top * c], n, u, 0)
            _ = m.vertex([bottom * s, -h, bottom * c], n, u, 1)
        }
        for j in 0..<segs {
            let (t0, b0, t1, b1) = (j * 2, j * 2 + 1, j * 2 + 2, j * 2 + 3)
            if top > 0 { m.triangle(t0, b0, t1) }
            m.triangle(t1, b0, b1)
        }
        for (radius, y, ny) in [(top, h, Float(1)), (bottom, -h, Float(-1))] where radius > 0 {
            let center = m.vertex([0, y, 0], [0, ny, 0], 0.5, 0.5)
            let first = center + 1
            for j in 0...segs {
                let (_, s, c) = around(j)
                _ = m.vertex([radius * s, y, radius * c], [0, ny, 0], 0.5 + 0.5 * s, 0.5 + 0.5 * c * -ny)
            }
            for j in 0..<segs {
                if ny > 0 { m.triangle(center, first + j, first + j + 1) } else { m.triangle(center, first + j + 1, first + j) }
            }
        }
        return m
    }

    /// In the XZ plane.
    static func torus(_ radius: Float, _ tube: Float) -> Self {
        var m = Self()
        let (segs, rings) = (sceneSegments, sceneRings)
        for i in 0...segs {
            let u = Float(i) / Float(segs)
            let phi = u * 2 * .pi
            let (s, c) = (sin(phi), cos(phi))
            for j in 0...rings {
                let v = Float(j) / Float(rings)
                let theta = v * 2 * .pi
                let (ts, tc) = (sin(theta), cos(theta))
                let n = PrismVec3(s * tc, ts, c * tc)
                let p = PrismVec3(s * (radius + tube * tc), tube * ts, c * (radius + tube * tc))
                _ = m.vertex(p, n, u, v)
            }
        }
        let row = rings + 1
        for i in 0..<segs {
            for j in 0..<rings {
                let (a, b) = (i * row + j, i * row + j + 1)
                let (c, d) = ((i + 1) * row + j, (i + 1) * row + j + 1)
                m.triangle(a, c, b)
                m.triangle(b, c, d)
            }
        }
        return m
    }

    /// Where the ray `origin + t · dir` crosses triangle `a b c` (either
    /// face), in front of the origin (Möller–Trumbore, edges widened by a
    /// hair like `geometry::ray_triangle`).
    static func rayTriangle(_ origin: PrismVec3, _ dir: PrismVec3, _ a: PrismVec3, _ b: PrismVec3, _ c: PrismVec3) -> Float? {
        let edge: Float = 1e-5
        let (e1, e2) = (b - a, c - a)
        let p = simd_cross(dir, e2)
        let det = simd_dot(e1, p)
        if abs(det) < 1e-9 { return nil }
        let inv = 1 / det
        let s = origin - a
        let u = simd_dot(s, p) * inv
        if !(-edge...(1 + edge)).contains(u) { return nil }
        let q = simd_cross(s, e1)
        let v = simd_dot(dir, q) * inv
        if v < -edge || u + v > 1 + edge { return nil }
        let t = simd_dot(e2, q) * inv
        return t > 1e-6 ? t : nil
    }
}

// MARK: Blobs (prism-core `scene3d::blobs` and `env`: fetched once by id, decoded once)

/// Rust's blob store (prism-native `prism_scene_blob`): calls `cb` at once
/// with the bytes, or with null if the blob is gone.
@_silgen_name("prism_scene_blob")
private func prismSceneBlobFetch(_ id: UnsafePointer<CChar>, _ ctx: UnsafeMutableRawPointer?,
                                 _ cb: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, Int) -> Void)

private final class PrismSceneBlobBox {
    var data: Data?
}

private let prismSceneBlobReceived: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, Int) -> Void = { ctx, bytes, len in
    guard let ctx, let bytes else { return }
    Unmanaged<PrismSceneBlobBox>.fromOpaque(ctx).takeUnretainedValue().data = Data(bytes: bytes, count: len)
}

/// Little-endian reader with 4-byte aligned sections (`blobs::Reader`):
/// every method returns nil past the end.
private struct PrismSceneReader {
    let data: Data
    var at = 0

    /// After checking the `P3D<kind>` header, version 1.
    init?(_ data: Data, kind: Character) {
        self.data = data
        guard let magic = take(4), data[magic] == Data("P3D".utf8) + Data([kind.asciiValue ?? 0]), u32() == 1 else {
            return nil
        }
    }

    init(raw data: Data) { self.data = data }

    mutating func take(_ n: Int) -> Range<Int>? {
        guard n >= 0, at <= data.count - n else { return nil }
        defer { at += n }
        return at..<(at + n)
    }

    mutating func u32() -> UInt32? {
        guard let r = take(4) else { return nil }
        return data.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: r.lowerBound, as: UInt32.self) }
    }

    mutating func u8() -> UInt8? { take(1).map { data[$0.lowerBound] } }

    mutating func f32() -> Float? { u32().map { Float(bitPattern: $0) } }

    private mutating func array<T>(_ n: Int, _: T.Type) -> [T]? {
        let size = MemoryLayout<T>.size
        guard n >= 0, n <= Int.max / size, let r = take(n * size) else { return nil }
        if n == 0 { return [] }
        return [T](unsafeUninitializedCapacity: n) { buf, count in
            data.withUnsafeBytes { src in
                _ = memcpy(buf.baseAddress!, src.baseAddress! + r.lowerBound, n * size)
            }
            count = n
        }
    }

    mutating func f32s(_ n: Int) -> [Float]? { array(n, Float.self) }
    mutating func u32s(_ n: Int) -> [UInt32]? { array(n, UInt32.self) }
    mutating func u16s(_ n: Int) -> [UInt16]? {
        let v = array(n, UInt16.self)
        align()
        return v
    }

    mutating func str() -> String? {
        guard let len = u32(), let r = take(Int(len)), let s = String(data: data[r], encoding: .utf8) else { return nil }
        align()
        return s
    }

    mutating func align() { at = min((at + 3) / 4 * 4, data.count) }

    mutating func bounds() -> PrismAabb? { f32s(6).map { PrismAabb(floats: $0) } }
}

/// An `I` blob: the instances' matrices (`T · R(q) · S`, relative to the
/// node), linear colors (or none) and their box.
final class PrismSceneInstances {
    let matrices: [PrismMat4]
    let colors: [SIMD4<Float>]
    let bounds: PrismAabb

    init(matrices: [PrismMat4], colors: [SIMD4<Float>], bounds: PrismAabb) {
        self.matrices = matrices
        self.colors = colors
        self.bounds = bounds
    }
}

/// One channel of an `A` blob (`ClipChannel`).
struct PrismSceneClipChannel {
    /// `""`, a child-index path or `"#name"`.
    var target: String
    /// 0 translation, 1 rotation, 2 scale.
    var property: UInt8
    /// 0 linear, 1 step, 2 cubic spline.
    var interpolation: UInt8
    var times: [Float]
    var values: [Float]

    var components: Int { property == 1 ? 4 : 3 }

    /// `ClipChannel::sample`: the value at `t` seconds (a unit quaternion
    /// for rotations).
    func sample(_ t: Float) -> SIMD4<Float> {
        let c = components
        let cubic = interpolation == 2
        let stride = cubic ? 3 * c : c
        let k = min(times.count, values.count / max(stride, 1))
        if k == 0 {
            switch property {
            case 1: return [0, 0, 0, 1]
            case 2: return [1, 1, 1, 0]
            default: return .zero
            }
        }
        func value(_ i: Int) -> SIMD4<Float> {
            let at = i * stride + (cubic ? c : 0)
            var v = SIMD4<Float>.zero
            for j in 0..<c { v[j] = values[at + j] }
            return v
        }
        func finish(_ v: SIMD4<Float>) -> SIMD4<Float> { property == 1 ? PrismSceneMath.quatNormalize(v) : v }
        if t <= times[0] || k == 1 { return finish(value(0)) }
        if t >= times[k - 1] { return finish(value(k - 1)) }
        // The key before t (times increase: binary search).
        var (lo, hi) = (0, k)
        while lo < hi {
            let mid = (lo + hi) / 2
            if times[mid] <= t { lo = mid + 1 } else { hi = mid }
        }
        let i = max(lo - 1, 0)
        let (t0, t1) = (times[i], times[i + 1])
        let dt = t1 - t0
        let s = dt > 0 ? (t - t0) / dt : 0
        switch interpolation {
        case 1:
            return finish(value(i))
        case 2:
            // glTF: p(s) = (2s³ − 3s² + 1)·v0 + (s³ − 2s² + s)·dt·b0
            //            + (−2s³ + 3s²)·v1 + (s³ − s²)·dt·a1
            let (s2, s3) = (s * s, s * s * s)
            let v0 = i * stride + c, b0 = i * stride + 2 * c
            let a1 = (i + 1) * stride, v1 = (i + 1) * stride + c
            var v = SIMD4<Float>.zero
            for j in 0..<c {
                let p0 = (2 * s3 - 3 * s2 + 1) * values[v0 + j] + (s3 - 2 * s2 + s) * dt * values[b0 + j]
                v[j] = p0 + (-2 * s3 + 3 * s2) * values[v1 + j] + (s3 - s2) * dt * values[a1 + j]
            }
            return finish(v)
        default:
            if property == 1 { return PrismSceneMath.slerp(value(i), value(i + 1), s) }
            let (a, b) = (value(i), value(i + 1))
            return a + (b - a) * s
        }
    }
}

/// An `A` blob (`ClipData`).
final class PrismSceneClip {
    let duration: Float
    let channels: [PrismSceneClipChannel]

    init(duration: Float, channels: [PrismSceneClipChannel]) {
        self.duration = duration
        self.channels = channels
    }
}

/// A `K` blob: inverse bind matrices and child-index joint paths from the
/// skin's root.
final class PrismSceneSkin {
    let inverseBind: [PrismMat4]
    let joints: [String]

    init(inverseBind: [PrismMat4], joints: [String]) {
        self.inverseBind = inverseBind
        self.joints = joints
    }
}

/// `f16_to_f32`.
private func prismSceneHalf(_ h: UInt16) -> Float {
    let sign: UInt32 = UInt32(h & 0x8000) << 16
    let e = Int((h >> 10) & 0x1f)
    let m = UInt32(h & 0x3ff)
    if e == 0 {
        let v = Float(m) * 5.9604645e-8 // m · 2^-24
        return sign != 0 ? -v : v
    }
    if e == 31 { return Float(bitPattern: sign | 0x7f80_0000 | (m << 13)) }
    return Float(bitPattern: sign | (UInt32(e - 15 + 127) << 23) | (m << 13))
}

/// ACES (Narkowicz' fit), as in `shaders.rs`.
private func prismSceneAces(_ x: Float) -> Float {
    min(max((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0), 1)
}

/// An `E` blob's sharpest level (the mirror image), as SceneKit images.
/// SceneKit's equirectangular images put the reference's `u` a quarter
/// turn further (`u_scenekit = u + 0.25`, measured): the columns are
/// rolled by a quarter of the width.
final class PrismSceneEnvMap {
    let width: Int
    let height: Int
    /// Linear RGB per texel of level 0, row 0 = +Y, in SceneKit's columns.
    private let rgb: [Float]
    /// The radiance as a float image (`lightingEnvironment`: SceneKit
    /// prefilters it).
    private(set) lazy var lighting: CGImage? = {
        var px = [Float](repeating: 1, count: width * height * 4)
        for i in 0..<(width * height) {
            px[i * 4] = rgb[i * 3]
            px[i * 4 + 1] = rgb[i * 3 + 1]
            px[i * 4 + 2] = rgb[i * 3 + 2]
        }
        let data = px.withUnsafeBufferPointer { Data(buffer: $0) } as CFData
        guard let space = CGColorSpace(name: CGColorSpace.extendedLinearSRGB),
              let provider = CGDataProvider(data: data) else { return nil }
        let info = CGBitmapInfo(rawValue: CGBitmapInfo.floatComponents.rawValue | CGImageAlphaInfo.last.rawValue
            | CGBitmapInfo.byteOrder32Little.rawValue)
        return CGImage(width: width, height: height, bitsPerComponent: 32, bitsPerPixel: 128, bytesPerRow: width * 16,
                       space: space, bitmapInfo: info, provider: provider, decode: nil, shouldInterpolate: true,
                       intent: .defaultIntent)
    }()
    private var shown: (Float, CGImage)?

    init?(_ data: Data) {
        var r = PrismSceneReader(raw: data)
        guard let magic = r.take(4), data[magic] == Data("P3DE".utf8), r.u32() == 1,
              let w = r.u32(), let h = r.u32(), let n = r.u32(), w > 0, h > 0, n > 0, n <= 32,
              r.f32s(27) != nil, let texels = r.u16s(Int(w) * Int(h) * 4) else { return nil }
        width = Int(w)
        height = Int(h)
        var rgb = [Float](repeating: 0, count: width * height * 3)
        let shift = width / 4
        for y in 0..<height {
            for x in 0..<width {
                let from = (y * width + x) * 4
                let to = (y * width + (x + shift) % width) * 3
                rgb[to] = prismSceneHalf(texels[from])
                rgb[to + 1] = prismSceneHalf(texels[from + 1])
                rgb[to + 2] = prismSceneHalf(texels[from + 2])
            }
        }
        self.rgb = rgb
    }

    /// The background: the sky program's `aces(c · scale)` then gamma, as
    /// sRGB bytes (`scale`: intensity × exposure).
    func background(_ scale: Float) -> CGImage? {
        if let (s, image) = shown, s == scale { return image }
        var px = [UInt8](repeating: 255, count: width * height * 4)
        for i in 0..<(width * height) {
            for c in 0..<3 {
                let v = pow(prismSceneAces(rgb[i * 3 + c] * scale), 1 / 2.2)
                px[i * 4 + c] = UInt8(min(max(v * 255 + 0.5, 0), 255))
            }
        }
        let data = Data(px) as CFData
        guard let space = CGColorSpace(name: CGColorSpace.sRGB), let provider = CGDataProvider(data: data),
              let image = CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
                                  bytesPerRow: width * 4, space: space,
                                  bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
                                  provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
        else { return nil }
        shown = (scale, image)
        return image
    }
}

/// Every blob the scenes use, fetched from Rust once and decoded once
/// (shared by every scene of the process; the same model in two scenes
/// shares its data and SceneKit buffers). Dropped when no live scene uses
/// it any more (`prune`).
final class PrismSceneBlobs {
    static let shared = PrismSceneBlobs()

    private var meshes: [String: PrismSceneMeshData] = [:]
    private var instances: [String: PrismSceneInstances] = [:]
    private var clips: [String: PrismSceneClip] = [:]
    private var skins: [String: PrismSceneSkin] = [:]
    private var envs: [String: PrismSceneEnvMap] = [:]
    private var images: [String: PrismSceneImage] = [:]

    /// A blob's bytes (nil if Rust no longer has it).
    func bytes(_ id: String) -> Data? {
        let box = PrismSceneBlobBox()
        id.withCString { prismSceneBlobFetch($0, Unmanaged.passUnretained(box).toOpaque(), prismSceneBlobReceived) }
        return box.data
    }

    /// `decode_mesh` (as `geometry::generate` lays it out: uvs zero when
    /// absent, whole triangles).
    func mesh(_ id: String) -> PrismSceneMeshData? {
        if let m = meshes[id] { return m }
        guard let data = bytes(id), var r = PrismSceneReader(data, kind: "M"),
              let n32 = r.u32(), let m32 = r.u32(), let flags = r.u32(), r.bounds() != nil else { return nil }
        let (n, m) = (Int(n32), Int(m32))
        guard n <= Int.max / 4, let positions = r.f32s(n * 3), let normals = r.f32s(n * 3) else { return nil }
        var mesh = PrismSceneMeshData()
        mesh.positions = positions
        mesh.normals = normals
        if flags & 1 != 0 {
            guard let uvs = r.f32s(n * 2) else { return nil }
            mesh.uvs = uvs
        } else {
            mesh.uvs = [Float](repeating: 0, count: n * 2)
        }
        if flags & 4 != 0 {
            guard let j = r.u16s(n * 4), let w = r.f32s(n * 4) else { return nil }
            mesh.joints = j
            mesh.weights = w
        }
        if flags & 8 != 0 {
            guard let c = r.f32s(n * 4) else { return nil }
            mesh.colors = c
        }
        guard var indices = r.u32s(m), !indices.contains(where: { Int($0) >= n }) else { return nil }
        indices.removeLast(indices.count % 3)
        mesh.indices = indices
        meshes[id] = mesh
        return mesh
    }

    /// `decode_instances`, with each instance's matrix.
    func instances(_ id: String) -> PrismSceneInstances? {
        if let s = instances[id] { return s }
        guard let data = bytes(id), var r = PrismSceneReader(data, kind: "I"),
              let count32 = r.u32(), let flags = r.u32(), let bounds = r.bounds() else { return nil }
        let count = Int(count32)
        guard count <= Int.max / 10, let raw = r.f32s(count * 10) else { return nil }
        let matrices = (0..<count).map { k -> PrismMat4 in
            let v = raw[(k * 10)..<(k * 10 + 10)].map { $0 }
            return PrismSceneMath.composeQuat([v[0], v[1], v[2]], [v[3], v[4], v[5], v[6]], [v[7], v[8], v[9]])
        }
        var colors: [SIMD4<Float>] = []
        if flags & 1 != 0 {
            guard let c = r.f32s(count * 4) else { return nil }
            colors = (0..<count).map { SIMD4(c[$0 * 4], c[$0 * 4 + 1], c[$0 * 4 + 2], c[$0 * 4 + 3]) }
        }
        let set = PrismSceneInstances(matrices: matrices, colors: colors, bounds: bounds)
        instances[id] = set
        return set
    }

    /// `decode_clip`.
    func clip(_ id: String) -> PrismSceneClip? {
        if let c = clips[id] { return c }
        guard let data = bytes(id), var r = PrismSceneReader(data, kind: "A"),
              let count = r.u32(), let duration = r.f32() else { return nil }
        var channels: [PrismSceneClipChannel] = []
        for _ in 0..<count {
            guard let target = r.str(), let property = r.u8(), property <= 2, let interp = r.u8(), interp <= 2,
                  r.take(2) != nil, let k32 = r.u32(), let times = r.f32s(Int(k32)) else { return nil }
            let perKey = (property == 1 ? 4 : 3) * (interp == 2 ? 3 : 1)
            guard let values = r.f32s(Int(k32) * perKey) else { return nil }
            channels.append(PrismSceneClipChannel(target: target, property: property, interpolation: interp,
                                                  times: times, values: values))
        }
        let clip = PrismSceneClip(duration: duration, channels: channels)
        clips[id] = clip
        return clip
    }

    /// `decode_skin`.
    func skin(_ id: String) -> PrismSceneSkin? {
        if let s = skins[id] { return s }
        guard let data = bytes(id), var r = PrismSceneReader(data, kind: "K"), let n32 = r.u32() else { return nil }
        let n = Int(n32)
        guard n <= Int.max / 16, let m = r.f32s(n * 16) else { return nil }
        let binds = (0..<n).map { i -> PrismMat4 in
            let c = { (k: Int) in SIMD4(m[i * 16 + k * 4], m[i * 16 + k * 4 + 1], m[i * 16 + k * 4 + 2], m[i * 16 + k * 4 + 3]) }
            return PrismMat4(columns: (c(0), c(1), c(2), c(3)))
        }
        var joints: [String] = []
        for _ in 0..<n {
            guard let s = r.str() else { return nil }
            joints.append(s)
        }
        let skin = PrismSceneSkin(inverseBind: binds, joints: joints)
        skins[id] = skin
        return skin
    }

    /// `EnvMap::decode` (its first level).
    func env(_ id: String) -> PrismSceneEnvMap? {
        if let e = envs[id] { return e }
        guard let data = bytes(id), let env = PrismSceneEnvMap(data) else { return nil }
        envs[id] = env
        return env
    }

    /// An embedded image (`p3d:<id>`).
    func image(_ id: String) -> PrismSceneImage? {
        if let i = images[id] { return i }
        guard let data = bytes(id), let image = PrismSceneImage(data: data) else { return nil }
        images[id] = image
        return image
    }

    /// Forgets the blobs no scene uses.
    func prune(keeping ids: Set<String>) {
        meshes = meshes.filter { ids.contains($0.key) }
        instances = instances.filter { ids.contains($0.key) }
        clips = clips.filter { ids.contains($0.key) }
        skins = skins.filter { ids.contains($0.key) }
        envs = envs.filter { ids.contains($0.key) }
        images = images.filter { ids.contains($0.key) }
    }
}

/// The triangles of every mesh: primitives generated once, blobs decoded
/// once (`ScenePlayer::geometry` and the SceneKit geometry share them).
final class PrismSceneMeshes {
    static let shared = PrismSceneMeshes()
    private var primitives: [String: PrismSceneMeshData] = [:]

    /// Empty while a blob mesh is missing (retried on the next call).
    func data(_ mesh: PrismSceneMesh, _ key: String) -> PrismSceneMeshData {
        if case let .blob(id, _) = mesh { return PrismSceneBlobs.shared.mesh(id) ?? PrismSceneMeshData() }
        if let m = primitives[key] { return m }
        let m = PrismSceneMeshData(primitive: mesh)
        primitives[key] = m
        return m
    }

    func prune(keeping keys: Set<String>) {
        primitives = primitives.filter { keys.contains($0.key) }
    }
}

// MARK: The player (prism-core `scene3d::player`, ported line by line)

/// Degrees the camera turns per point dragged.
private let sceneDegreesPerPoint: Float = 0.5
/// The camera stays this many degrees away from straight up or down.
private let sceneMaxPitch: Float = 89
/// Width and height of the shadow map, texels.
let prismSceneShadowMapSize = 2048
/// Most lights a draw uses (`shaders::MAX_LIGHTS`; ambient ones apart).
private let sceneMaxLights = 8

/// The eased progress at `p` (0 to 1) of a CSS `cubic-bezier` curve.
func prismSceneEase(_ bezier: SIMD4<Float>, _ p: Float) -> Float {
    let p = min(max(p, 0), 1)
    if p == 0 || p == 1 { return p }
    func at(_ a: Float, _ b: Float, _ t: Float) -> Float {
        let u = 1 - t
        return 3 * u * u * t * a + 3 * u * t * t * b + t * t * t
    }
    var (lo, hi): (Float, Float) = (0, 1)
    for _ in 0..<24 {
        let mid = (lo + hi) / 2
        if at(bezier.x, bezier.z, mid) < p { lo = mid } else { hi = mid }
    }
    return at(bezier.y, bezier.w, (lo + hi) / 2)
}

/// The animatable part of a node.
private struct PrismScenePose: Equatable {
    var position: PrismVec3
    var rotation: PrismVec3
    var quat: PrismQuat?
    var scale: PrismVec3
    var color: PrismSceneRgba
    var emissive: PrismSceneRgba
    var metalness: Float
    var roughness: Float

    init(_ n: PrismSceneFlatNode) {
        position = n.position
        rotation = n.rotation
        quat = n.quaternion
        scale = n.scale
        color = n.material.color
        emissive = n.material.emissive ?? .black
        metalness = n.material.metalness
        roughness = n.material.roughness
    }

    func lerp(_ to: PrismScenePose, _ t: Float) -> PrismScenePose {
        var p = self
        if quat == nil && to.quat == nil {
            // Degrees, like v1 and 2D `rotate`: every degree in between.
            p.rotation = rotation + (to.rotation - rotation) * t
        } else {
            let a = quat ?? PrismSceneMath.quatFromEuler(rotation)
            let b = to.quat ?? PrismSceneMath.quatFromEuler(to.rotation)
            p.rotation = to.rotation
            p.quat = PrismSceneMath.slerp(a, b, t)
        }
        p.position = position + (to.position - position) * t
        p.scale = scale + (to.scale - scale) * t
        p.color = color.lerp(to.color, t)
        p.emissive = emissive.lerp(to.emissive, t)
        p.metalness = PrismSceneMath.lerp(metalness, to.metalness, t)
        p.roughness = PrismSceneMath.lerp(roughness, to.roughness, t)
        return p
    }
}

/// What clips set on a node (each part replaces the node's own).
private struct PrismSceneClipPose: Equatable {
    var translation: PrismVec3?
    var rotation: PrismQuat?
    var scale: PrismVec3?
}

/// A clip playing on a node.
private final class PrismSceneTrack {
    let clip: PrismSceneClip
    /// Clip time at `anchor`.
    var base: Float
    var anchor: Double
    var speed: Float
    var looping: Bool
    /// The node each channel animates (resolved after every update).
    var targets: [Int?] = []

    init(_ clip: PrismSceneClip, _ now: Double, _ speed: Float, _ looping: Bool) {
        self.clip = clip
        base = speed < 0 ? clip.duration : 0
        anchor = now
        self.speed = speed
        self.looping = looping
    }

    /// Unbounded clip time at `now`.
    func rawTime(_ now: Double) -> Float { base + Float(now - anchor) * speed }

    /// The time to sample at `now`.
    func time(_ now: Double) -> Float {
        let d = clip.duration
        let t = rawTime(now)
        if d <= 0 { return 0 }
        if looping {
            let r = t.truncatingRemainder(dividingBy: d)
            return r < 0 ? r + d : r
        }
        return min(max(t, 0), d)
    }

    /// Still moving at `now` (looping, or not past its end).
    func running(_ now: Double) -> Bool {
        let t = rawTime(now)
        return clip.duration > 0 && speed != 0
            && (looping || (speed > 0 && t < clip.duration) || (speed < 0 && t > 0))
    }

    /// Changes the rate without a jump in clip time.
    func setSpeed(_ speed: Float, _ now: Double) {
        if speed != self.speed {
            base = rawTime(now)
            anchor = now
            self.speed = speed
        }
    }

    /// Adds this clip's values at `now` to `out` (by target node).
    func sample(_ now: Double, into out: inout [Int: PrismSceneClipPose]) {
        let t = time(now)
        for (c, target) in zip(clip.channels, targets) {
            guard let target else { continue }
            let v = c.sample(t)
            var pose = out[target] ?? PrismSceneClipPose()
            switch c.property {
            case 0: pose.translation = PrismVec3(v.x, v.y, v.z)
            case 1: pose.rotation = v
            default: pose.scale = PrismVec3(v.x, v.y, v.z)
            }
            out[target] = pose
        }
    }
}

/// The clips of a node with `play`.
private final class PrismScenePlaying {
    /// The clip playing (nil while its blob is missing).
    var current: PrismSceneTrack?
    /// The clip blending out: (track, fade start, fade seconds).
    var previous: (PrismSceneTrack, Double, Float)?
    /// It moved at the last refresh: sample once more after it stops.
    var live: Bool

    init(current: PrismSceneTrack?, previous: (PrismSceneTrack, Double, Float)?, live: Bool) {
        self.current = current
        self.previous = previous
        self.live = live
    }
}

/// The camera as an orbit around its target.
struct PrismSceneOrbitState: Equatable {
    /// Degrees around Y; 0: on +Z of the target.
    var yaw: Float
    /// Degrees above the horizon.
    var pitch: Float
    var distance: Float
    var target: PrismVec3
    var fov: Float
    /// Visible height of an orthographic camera.
    var ortho: Float?

    init(_ c: PrismSceneCamera) {
        let o = c.position - c.target
        let d = max(simd_length(o), 1e-4)
        yaw = PrismSceneMath.deg(atan2(o.x, o.z))
        pitch = PrismSceneMath.deg(asin(min(max(o.y / d, -1), 1)))
        distance = d
        target = c.target
        fov = c.fov
        ortho = c.ortho
    }

    var eye: PrismVec3 {
        let (y, p) = (PrismSceneMath.rad(yaw), PrismSceneMath.rad(pitch))
        return target + distance * PrismVec3(cos(p) * sin(y), sin(p), cos(p) * cos(y))
    }

    func lerp(_ to: PrismSceneOrbitState, _ t: Float) -> PrismSceneOrbitState {
        // The shorter way around.
        let r = (to.yaw - yaw + 540).truncatingRemainder(dividingBy: 360)
        let dyaw = (r < 0 ? r + 360 : r) - 180
        var o = self
        o.yaw = yaw + dyaw * t
        o.pitch = PrismSceneMath.lerp(pitch, to.pitch, t)
        o.distance = PrismSceneMath.lerp(distance, to.distance, t)
        o.target = target + (to.target - target) * t
        o.fov = PrismSceneMath.lerp(fov, to.fov, t)
        if let a = ortho, let b = to.ortho { o.ortho = PrismSceneMath.lerp(a, b, t) } else { o.ortho = to.ortho }
        return o
    }
}

/// One light of a frame (`FrameLight`, with the light it comes from).
struct PrismSceneFrameLight {
    var light: PrismSceneLight
    /// 1 directional, 2 point, 3 spot.
    var kind: Int
    /// Unit direction the light travels.
    var direction: PrismVec3
}

/// A material resolved for one frame (`DrawMaterial`).
struct PrismSceneDrawMaterial: Equatable {
    /// Linear rgb and alpha (1 when opaque).
    var color: SIMD4<Float>
    /// Linear.
    var emissive: PrismVec3
    var metalness: Float
    var roughness: Float
    var unlit: Bool
    var doubleSided: Bool
    var texture: String?
    var mrMap: String?
    var normalMap: String?
    var normalScale: Float
    var emissiveMap: String?
    var aoMap: String?
    var alpha: PrismSceneAlpha
    var cutoff: Float
}

/// One mesh to draw.
struct PrismSceneDraw {
    /// The node's row in the player.
    var node: Int
    var key: String
    /// The nearest named ancestor's name (itself included).
    var name: String?
    var mesh: PrismSceneMesh
    var meshKey: String
    /// Model matrix (scene coordinates).
    var world: PrismMat4
    var material: PrismSceneDrawMaterial
    /// Blended (alpha below 1): after the opaque meshes, no depth writes.
    var transparent: Bool
    /// The `I` blob and its count.
    var instances: (id: String, count: Int)?
    /// Placed by its joints (`Draw::joints`).
    var skinned: Bool
    var castShadows: Bool
    var receiveShadows: Bool
    /// Its box in scene coordinates.
    var bounds: PrismAabb
}

/// The shadow map of a frame (`ShadowPass`), with the light camera it
/// comes from (SceneKit's shadow light takes it).
struct PrismSceneShadowPass {
    /// An index into the frame's lights.
    var light: Int
    var viewProj: PrismMat4
    /// The light's view matrix, half the map's height (world units) and its
    /// depth range (distances along the view's −Z).
    var view: PrismMat4
    var half: Float
    var near: Float
    var far: Float
    var mapSize: Int
    var bias: Float
    var normalBias: Float
    /// Indices into the frame's draws.
    var casters: [Int]
}

/// The environment of a frame.
struct PrismSceneFrameEnv: Equatable {
    var id: String
    var intensity: Float
    var background: Bool
}

/// Everything one frame draws.
struct PrismSceneFrame {
    var width: Float
    var height: Float
    var view: PrismMat4
    var projection: PrismMat4
    var viewProj: PrismMat4
    var inverseViewProj: PrismMat4
    var cameraPosition: PrismVec3
    /// The camera at this frame.
    var orbit: PrismSceneOrbitState
    /// Ambient lights (summed by the reference).
    var ambient: [PrismSceneLight]
    /// The other lights (at most 8).
    var lights: [PrismSceneFrameLight]
    /// Opaque meshes, then transparent ones back to front; culled ones left out.
    var draws: [PrismSceneDraw]
    var shadow: PrismSceneShadowPass?
    var env: PrismSceneFrameEnv?
    var exposure: Float
    /// Something moves: draw another frame.
    var animating: Bool
}

/// A tap's `SceneTapEvent`.
struct PrismSceneTap {
    var node: String?
    var part: String?
    var instance: Int?
    var point: PrismVec3?

    /// `{"node", "part", "instance", "point"}`.
    var fields: [String: Any] {
        ["node": node ?? NSNull(), "part": part ?? NSNull(), "instance": instance ?? NSNull(),
         "point": point.map { [Double($0.x), Double($0.y), Double($0.z)] } ?? NSNull()]
    }

    var json: String {
        let data = (try? JSONSerialization.data(withJSONObject: fields)) ?? Data("{}".utf8)
        return String(data: data, encoding: .utf8) ?? "{}"
    }
}

/// A row of the node table.
final class PrismSceneRow {
    var flat: PrismSceneFlatNode
    var parent: Int?
    /// Its subtree is `index..<end`.
    var end = 0
    var children: [Int] = []
    /// The nearest named node, itself included.
    var named: Int?
    /// The nearest labeled node up to the named one.
    var part: Int?
    /// Neither it nor an ancestor is hidden.
    var visible = true
    var meshKey: String
    /// When it appeared (its spin starts there).
    let appeared: Double
    fileprivate var target: PrismScenePose
    fileprivate var from: (PrismScenePose, Double)?
    fileprivate var clipPose: PrismSceneClipPose?
    fileprivate var playing: PrismScenePlaying?
    /// The joint nodes of its skin (resolved after every update).
    var jointNodes: [Int?] = []
    /// `inverse(world) · world(joint i) · inverse_bind[i]`.
    var joints: [PrismMat4] = []
    /// `world(joint i) · inverse_bind[i]` (SceneKit's bone matrices).
    var placed: [PrismMat4] = []
    /// The tick its joints last changed.
    var skinTick: UInt64 = 0
    var local = matrix_identity_float4x4
    var world = matrix_identity_float4x4
    /// Its draw's box in scene coordinates.
    var bounds = PrismAabb.zero
    /// The tick its world last changed.
    var moved: UInt64 = 0
    /// Its local matrix needs recomputing.
    var dirty = true

    init(_ flat: PrismSceneFlatNode, _ now: Double) {
        meshKey = flat.mesh?.key ?? ""
        target = PrismScenePose(flat)
        self.flat = flat
        appeared = now
    }
}

/// The device side of one scene element (see prism-core's `player.rs`):
/// `apply` on every update, gestures, then `advance` + `frame` each display
/// frame while `animating`, `hit` on taps.
final class PrismScenePlayer {
    private(set) var settings = PrismSceneSettings()
    /// No update applied yet: the first one does not animate.
    private var fresh = true
    private(set) var rows: [PrismSceneRow] = []
    private var index: [String: Int] = [:]
    /// Visible nodes with a mesh.
    private var drawNodes: [Int] = []
    private var transitioning: [Int] = []
    private var spinning: [Int] = []
    private var players: [Int] = []
    /// Visible nodes with a resolved skin.
    private var skinned: [Int] = []
    private var orbit: PrismSceneOrbitState
    private var cameraFrom: (PrismSceneOrbitState, Double)?
    private var minDistance: Float = 0
    private var maxDistance: Float = 0
    private var now: Double
    private var last: Double?
    private var dragging = false
    /// Something but the camera moves (spin, transition, clip).
    private var moving = false
    private(set) var tick: UInt64 = 0

    /// A press that moves less than this (points) is a tap.
    static let tapSlop: Float = 6

    /// Seconds, any origin.
    static func clock() -> Double { ProcessInfo.processInfo.systemUptime }

    init(now: Double) {
        orbit = PrismSceneOrbitState(PrismSceneCamera())
        self.now = now
    }

    /// Shows an update at `now` (`ScenePlayer::apply`).
    func apply(_ update: PrismSceneUpdate, now: Double) {
        self.now = now
        if let s = update.settings { setSettings(s, now) }
        let t = fresh ? nil : settings.transition
        fresh = false

        let oldOrder = rows.map(\.flat.key)
        var byKey: [String: PrismSceneRow] = [:]
        for r in rows { byKey[r.flat.key] = r }
        rows = []
        // Keys in their new order (removed ones simply find no row).
        let order: [String]
        switch update {
        case let .full(_, nodes):
            order = nodes.map(\.key)
        case let .diff(_, upsert, remove, o):
            for k in remove { byKey[k] = nil }
            order = o ?? (oldOrder + upsert.map(\.key))
        }
        for flat in update.nodes {
            if let row = byKey[flat.key] {
                updateRow(row, flat, now, t)
            } else {
                let row = PrismSceneRow(flat, now)
                startPlay(row, old: nil, now)
                byKey[flat.key] = row
            }
        }
        var seen = Set<String>()
        rows = order.compactMap { k in seen.insert(k).inserted ? byKey.removeValue(forKey: k) : nil }
        relink()
        refresh(now, all: true)
    }

    private func setSettings(_ s: PrismSceneSettings, _ now: Double) {
        if fresh {
            orbit = PrismSceneOrbitState(s.camera)
            setLimits(s)
        } else if s.camera != settings.camera {
            let to = PrismSceneOrbitState(s.camera)
            let from = currentOrbit(now)
            orbit = to
            cameraFrom = s.transition.flatMap { $0.ms > 0 ? (from, now + Double($0.delay) / 1000) : nil }
            setLimits(s)
        } else if s.orbit != settings.orbit {
            setLimits(s)
        }
        settings = s
    }

    private func setLimits(_ s: PrismSceneSettings) {
        let d = PrismSceneOrbitState(s.camera).distance
        let o = s.orbit ?? PrismSceneOrbit()
        minDistance = o.min > 0 ? o.min : d * 0.25
        maxDistance = o.max > 0 ? o.max : d * 4
    }

    /// A node of the update replaces its row's props.
    private func updateRow(_ row: PrismSceneRow, _ flat: PrismSceneFlatNode, _ now: Double, _ t: PrismSceneTransition?) {
        let pose = PrismScenePose(flat)
        if pose != row.target {
            let current = poseAt(row, now)
            if let t, t.ms > 0, current != pose {
                row.from = (current, now + Double(t.delay) / 1000)
            } else {
                row.from = nil
            }
            row.target = pose
        }
        if flat.mesh != row.flat.mesh { row.meshKey = flat.mesh?.key ?? "" }
        let old = row.flat
        row.flat = flat
        startPlay(row, old: old.play, now)
    }

    /// Starts, restarts, crossfades, retimes or stops a row's clip after
    /// its `play` changed from `old`.
    private func startPlay(_ row: PrismSceneRow, old: PrismScenePlayRef?, _ now: Double) {
        guard let p = row.flat.play else {
            row.playing = nil
            return
        }
        let clip = PrismSceneBlobs.shared.clip(p.clip)
        let same = old.map { $0.clip == p.clip && $0.run == p.run } ?? false
        if let playing = row.playing, same {
            if let track = playing.current {
                track.setSpeed(p.speed, now)
                track.looping = p.looping
            }
        } else {
            var previous: (PrismSceneTrack, Double, Float)?
            if let old = row.playing, p.fade > 0, let track = old.current { previous = (track, now, p.fade) }
            row.playing = PrismScenePlaying(current: clip.map { PrismSceneTrack($0, now, p.speed, p.looping) },
                                            previous: previous, live: true)
        }
    }

    /// Rebuilds the table's links after an update (`ScenePlayer::relink`).
    private func relink() {
        // Pre-order from the parent keys, whatever order the rows came in
        // (rows whose parent is missing go to the top level).
        var keys: [String: Int] = [:]
        for (i, r) in rows.enumerated() { keys[r.flat.key] = i }
        var kids = [[Int]](repeating: [], count: rows.count)
        var roots: [Int] = []
        for (i, r) in rows.enumerated() {
            if let pk = r.flat.parent, let p = keys[pk], p != i { kids[p].append(i) } else { roots.append(i) }
        }
        var order: [Int] = []
        order.reserveCapacity(rows.count)
        var stack = Array(roots.reversed())
        var placed = [Bool](repeating: false, count: rows.count)
        while let i = stack.popLast() {
            if placed[i] { continue }
            placed[i] = true
            order.append(i)
            stack.append(contentsOf: kids[i].reversed())
        }
        if order.count != rows.count || order.enumerated().contains(where: { $0.offset != $0.element }) {
            let old = rows
            rows = order.map { old[$0] }
        }

        index = [:]
        for (i, r) in rows.enumerated() { index[r.flat.key] = i }
        let n = rows.count
        for i in 0..<n {
            let row = rows[i]
            let parent = row.flat.parent.flatMap { index[$0] }.flatMap { $0 < i ? $0 : nil }
            let (named, part, visible): (Int?, Int?, Bool) = parent.map { (rows[$0].named, rows[$0].part, rows[$0].visible) }
                ?? (nil, nil, true)
            row.parent = parent
            row.end = i + 1
            row.children = []
            row.named = row.flat.name != nil ? i : named
            row.part = row.flat.label != nil ? i : (row.flat.name != nil ? nil : part)
            row.visible = visible && !row.flat.hidden
            row.clipPose = nil
            row.dirty = true
        }
        for i in (0..<n).reversed() {
            if let p = rows[i].parent { rows[p].end = max(rows[p].end, rows[i].end) }
        }
        for i in 0..<n {
            if let p = rows[i].parent { rows[p].children.append(i) }
        }

        // Clip targets and skin joints.
        for i in 0..<n {
            if let playing = rows[i].playing {
                for track in [playing.current, playing.previous?.0].compactMap({ $0 }) {
                    track.targets = track.clip.channels.map { resolve(i, $0.target) }
                }
            }
            if let s = rows[i].flat.skin, let skin = PrismSceneBlobs.shared.skin(s.id), let root = index[s.root] {
                rows[i].jointNodes = skin.joints.map { resolve(root, $0) }
            } else {
                rows[i].jointNodes = []
            }
        }

        drawNodes = (0..<n).filter { rows[$0].visible && rows[$0].flat.mesh != nil }
        spinning = (0..<n).filter { rows[$0].visible && rows[$0].flat.spin != .zero }
        transitioning = (0..<n).filter { rows[$0].from != nil }
        players = (0..<n).filter { rows[$0].playing != nil }
        skinned = (0..<n).filter { rows[$0].visible && !rows[$0].jointNodes.isEmpty }
    }

    /// The node a clip target or joint path names, from node `from`: `""`
    /// itself, `"0/2"` a child-index path, `"#name"` a named node.
    private func resolve(_ from: Int, _ target: String) -> Int? {
        if target.hasPrefix("#") { return index[target] }
        var at = from
        for step in target.split(separator: "/") where !step.isEmpty {
            guard let k = Int(step), k >= 0, k < rows[at].children.count else { return nil }
            at = rows[at].children[k]
        }
        return at
    }

    private func transitionProgress(_ start: Double, _ now: Double) -> Float? {
        guard let t = settings.transition, t.ms > 0 else { return nil }
        let p = Float((now - start) * 1000 / Double(t.ms))
        return p < 1 ? prismSceneEase(t.bezier, max(p, 0)) : nil
    }

    private func poseAt(_ n: PrismSceneRow, _ now: Double) -> PrismScenePose {
        guard let (from, start) = n.from, let p = transitionProgress(start, now) else { return n.target }
        return from.lerp(n.target, p)
    }

    private func currentOrbit(_ now: Double) -> PrismSceneOrbitState {
        guard let (from, start) = cameraFrom, let p = transitionProgress(start, now) else { return orbit }
        return from.lerp(orbit, p)
    }

    /// The finger or the mouse button went down (auto-rotation pauses).
    func pointerDown() { dragging = true }

    /// The pointer went up.
    func pointerUp() { dragging = false }

    /// Moved by `dx`, `dy` points while down: turns the camera.
    func drag(_ dx: Float, _ dy: Float) {
        guard settings.orbit?.rotate == true else { return }
        settleCamera()
        orbit.yaw -= dx * sceneDegreesPerPoint
        orbit.pitch = min(max(orbit.pitch + dy * sceneDegreesPerPoint, -sceneMaxPitch), sceneMaxPitch)
    }

    /// Pinch or scroll: `factor` above 1 moves closer.
    func zoom(_ factor: Float) {
        guard settings.orbit?.zoom == true, !factor.isNaN, factor > 0 else { return }
        settleCamera()
        if let h = orbit.ortho {
            orbit.ortho = h / factor
        } else {
            orbit.distance = min(max(orbit.distance / factor, minDistance), maxDistance)
        }
    }

    /// Two-finger drag: moves the target with the content.
    func pan(_ dx: Float, _ dy: Float, height: Float) {
        guard settings.orbit?.pan == true, height > 0 else { return }
        settleCamera()
        let eye = orbit.eye
        let f = PrismSceneMath.normalize(orbit.target - eye)
        let right = PrismSceneMath.normalize(simd_cross(f, [0, 1, 0]))
        let up = simd_cross(right, f)
        let visible = orbit.ortho ?? 2 * orbit.distance * tan(PrismSceneMath.rad(orbit.fov) / 2)
        let k = visible / height
        orbit.target += right * (-dx * k) + up * (dy * k)
    }

    /// A gesture takes over from a running camera transition.
    private func settleCamera() {
        if cameraFrom != nil {
            orbit = currentOrbit(now)
            cameraFrom = nil
        }
    }

    /// Moves time to `now`: auto-rotation, spins, transitions and clips.
    func advance(_ now: Double) {
        let dt = Float(last.map { min(max(now - $0, 0), 0.25) } ?? 0)
        last = now
        self.now = now
        if let o = settings.orbit, o.auto != 0, !dragging {
            orbit.yaw += o.auto * dt
            cameraFrom?.0.yaw += o.auto * dt
        }
        if let (_, start) = cameraFrom, now >= start, transitionProgress(start, now) == nil {
            cameraFrom = nil
        }
        refresh(now, all: false)
    }

    /// Recomputes what moved at `now`: clip poses, then the local matrices
    /// of moving nodes and the worlds of their subtrees, then skins. `all`:
    /// every node (after an update).
    private func refresh(_ now: Double, all: Bool) {
        tick += 1
        let tick = self.tick
        var moving = false
        // Rows whose local pose changes now.
        var touched: [Int] = []

        // Fades that are over: the nodes only the old clip moved go back
        // to their own pose.
        for p in players {
            guard let playing = rows[p].playing, let (track, start, fade) = playing.previous,
                  now - start >= Double(fade) else { continue }
            playing.previous = nil
            playing.live = true
            for t in track.targets {
                guard let t else { continue }
                rows[t].clipPose = nil
                rows[t].dirty = true
                touched.append(t)
            }
        }
        // Clips: sample every running track (and, after an update, the
        // finished ones too: their last pose holds).
        var poses: [Int: PrismSceneClipPose] = [:]
        var live: [(Int, Bool)] = []
        for p in players {
            guard let playing = rows[p].playing else { continue }
            let running = playing.previous != nil || (playing.current?.running(now) ?? false)
            live.append((p, running))
            if !running && !playing.live && !all { continue }
            moving = moving || running
            var new: [Int: PrismSceneClipPose] = [:]
            playing.current?.sample(now, into: &new)
            if let (oldTrack, start, fade) = playing.previous {
                let w = min(max(Float((now - start) / Double(fade)), 0), 1)
                var old: [Int: PrismSceneClipPose] = [:]
                oldTrack.sample(now, into: &old)
                for (target, a) in old {
                    let b = new.removeValue(forKey: target) ?? PrismSceneClipPose()
                    poses[target] = blend(target, a, b, w, now)
                }
                for (target, b) in new { poses[target] = blend(target, PrismSceneClipPose(), b, w, now) }
            } else {
                poses.merge(new) { _, n in n }
            }
        }
        for (p, running) in live { rows[p].playing?.live = running }
        for (target, pose) in poses where rows[target].clipPose != pose {
            rows[target].clipPose = pose
            rows[target].dirty = true
            touched.append(target)
        }

        // Spins and transitions.
        for i in spinning {
            rows[i].dirty = true
            touched.append(i)
            moving = true
        }
        var done: [Int] = []
        for i in transitioning {
            rows[i].dirty = true
            touched.append(i)
            if let (_, start) = rows[i].from, now < start || transitionProgress(start, now) != nil {
                moving = true
            } else {
                done.append(i)
            }
        }
        if !done.isEmpty {
            for i in done { rows[i].from = nil }
            transitioning.removeAll { done.contains($0) }
        }

        // Locals, then the worlds of the subtrees of the touched rows (in
        // pre-order, a subtree is a range: one pass covers nested ones).
        if all {
            touched = Array(0..<rows.count)
        } else {
            touched.sort()
        }
        var covered = 0
        for i in touched where i >= covered {
            covered = rows[i].end
            for j in i..<covered {
                if all || rows[j].dirty {
                    rows[j].local = local(j, now)
                    rows[j].dirty = false
                }
                updateWorld(j, tick)
            }
        }

        // Skins whose joints or node moved.
        for s in skinned {
            let row = rows[s]
            let moved = all || row.moved == tick || row.jointNodes.contains { $0.map { rows[$0].moved == tick } ?? false }
            guard moved, let k = row.flat.skin, let skin = PrismSceneBlobs.shared.skin(k.id) else { continue }
            // Where each joint puts the mesh: world · J_i = world(joint i) ·
            // inverse_bind[i] (the node's own transform places nothing).
            let placed = row.jointNodes.enumerated().map { k, j -> PrismMat4 in
                guard let j else { return row.world }
                let bind = k < skin.inverseBind.count ? skin.inverseBind[k] : matrix_identity_float4x4
                return rows[j].world * bind
            }
            let inverse = PrismSceneMath.invert(row.world)
            row.joints = placed.map { inverse * $0 }
            row.placed = placed
            row.skinTick = tick
            // Every skinned vertex is a weighted mean of `world · J_i · v`, so
            // the union of the mesh box under each `world · J_i` holds them.
            if let mesh = row.flat.mesh, row.flat.instances == nil, let first = placed.first {
                let b = mesh.bounds
                row.bounds = placed.dropFirst().reduce(b.transformed(first)) { $0.union(b.transformed($1)) }
            }
        }
        self.moving = moving
    }

    /// Blends two clips' poses of node `target` (`w`: the new one's
    /// weight); a part only one clip sets blends with the node's own pose.
    private func blend(_ target: Int, _ a: PrismSceneClipPose, _ b: PrismSceneClipPose, _ w: Float, _ now: Double) -> PrismSceneClipPose {
        let own = poseAt(rows[target], now)
        let ownQ = own.quat ?? PrismSceneMath.quatFromEuler(own.rotation)
        func mix3(_ x: PrismVec3?, _ y: PrismVec3?, _ base: PrismVec3) -> PrismVec3? {
            guard x != nil || y != nil else { return nil }
            let (p, q) = (x ?? base, y ?? base)
            return p + (q - p) * w
        }
        var out = PrismSceneClipPose()
        out.translation = mix3(a.translation, b.translation, own.position)
        if a.rotation != nil || b.rotation != nil {
            out.rotation = PrismSceneMath.slerp(a.rotation ?? ownQ, b.rotation ?? ownQ, w)
        }
        out.scale = mix3(a.scale, b.scale, own.scale)
        return out
    }

    /// Row `i`'s local matrix at `now`.
    private func local(_ i: Int, _ now: Double) -> PrismMat4 {
        let row = rows[i]
        let pose = poseAt(row, now)
        let clip = row.clipPose ?? PrismSceneClipPose()
        let position = clip.translation ?? pose.position
        let scale = clip.scale ?? pose.scale
        let spin = row.flat.spin
        let turned = spin != .zero && row.visible ? spin * Float(now - row.appeared) : .zero
        if let q = clip.rotation ?? pose.quat {
            let q = turned != .zero ? PrismSceneMath.quatMul(q, PrismSceneMath.quatFromEuler(turned)) : q
            return PrismSceneMath.composeQuat(position, q, scale)
        }
        return PrismSceneMath.compose(position, pose.rotation + turned, scale)
    }

    /// Row `i`'s world matrix from its parent's, and its box.
    private func updateWorld(_ i: Int, _ tick: UInt64) {
        let row = rows[i]
        let world = row.parent.map { rows[$0].world * row.local } ?? row.local
        if row.world != world || row.moved == 0 {
            row.world = world
            row.moved = tick
        }
        // A skinned mesh's box follows its joints (set with them, after the
        // worlds); until they resolve, its node places it.
        let skinned = !row.jointNodes.isEmpty && row.flat.instances == nil
        if let mesh = row.flat.mesh, !skinned {
            row.bounds = (row.flat.instances?.bounds ?? mesh.bounds).transformed(world)
        }
    }

    /// The frame at the time of the last `advance`, for a viewport of
    /// `width` × `height` points.
    func frame(_ width: Float, _ height: Float, dark: Bool) -> PrismSceneFrame {
        let cam = currentOrbit(now)
        let eye = cam.eye
        let aspect = height > 0 ? width / height : 1
        let c = settings.camera
        let projection = cam.ortho.map { PrismSceneMath.orthographic($0, aspect, c.near, c.far) }
            ?? PrismSceneMath.perspective(cam.fov, aspect, c.near, c.far)
        let view = PrismSceneMath.lookAt(eye, cam.target, [0, 1, 0])
        let viewProj = projection * view

        var ambient: [PrismSceneLight] = []
        var lights: [PrismSceneFrameLight] = []
        var shadowLight: Int?
        for l in settings.effectiveLights {
            let kind: Int
            switch l.kind {
            case .ambient:
                ambient.append(l)
                continue
            case .directional: kind = 1
            case .point: kind = 2
            case .spot: kind = 3
            }
            if lights.count < sceneMaxLights {
                if kind == 1 && l.shadows && shadowLight == nil { shadowLight = lights.count }
                let d = PrismSceneMath.normalize(l.direction)
                lights.append(PrismSceneFrameLight(light: l, kind: kind, direction: d == .zero ? [0, -1, 0] : d))
            }
        }

        let planes = PrismSceneMath.frustumPlanes(viewProj)
        var opaque: [PrismSceneDraw] = []
        var transparent: [PrismSceneDraw] = []
        for i in drawNodes {
            guard let draw = draw(i, dark), PrismSceneMath.aabbInFrustum(planes, draw.bounds) else { continue }
            if draw.transparent { transparent.append(draw) } else { opaque.append(draw) }
        }
        // Fewer program, texture and buffer switches.
        opaque.sort { a, b in
            let va = (a.instances != nil ? 1 : 0, a.skinned ? 1 : 0), vb = (b.instances != nil ? 1 : 0, b.skinned ? 1 : 0)
            if va != vb { return va < vb }
            if a.material.texture != b.material.texture {
                guard let ta = a.material.texture else { return true }
                guard let tb = b.material.texture else { return false }
                return ta < tb
            }
            return a.meshKey < b.meshKey
        }
        // Back to front: farthest from the camera first.
        func depth(_ d: PrismSceneDraw) -> Float { simd_length(d.bounds.center - eye) }
        transparent.sort { depth($0) > depth($1) }
        let draws = opaque + transparent

        let shadow = shadowLight.flatMap { prismSceneShadowPass($0, lights[$0], draws) }
        let env = settings.env.map { PrismSceneFrameEnv(id: $0.id, intensity: $0.intensity, background: $0.background) }
        let animating = moving || cameraFrom != nil || (settings.orbit.map { $0.auto != 0 } ?? false) && !dragging
        return PrismSceneFrame(width: width, height: height, view: view, projection: projection, viewProj: viewProj,
                               inverseViewProj: PrismSceneMath.invert(viewProj), cameraPosition: eye, orbit: cam,
                               ambient: ambient, lights: lights, draws: draws, shadow: shadow, env: env,
                               exposure: settings.exposure, animating: animating)
    }

    /// Row `i`'s draw (nil: invisible, alpha 0).
    private func draw(_ i: Int, _ dark: Bool) -> PrismSceneDraw? {
        let row = rows[i]
        guard let mesh = row.flat.mesh else { return nil }
        let m = row.flat.material
        let pose = poseAt(row, now)
        let color = pose.color.resolve(dark)
        let e = pose.emissive.resolve(dark)
        let a = m.alpha == .opaque ? 1 : color.w
        // Invisible: neither drawn nor hit.
        if a <= 0 { return nil }
        let lin = PrismSceneMath.srgbToLinear
        let material = PrismSceneDrawMaterial(
            color: [lin(color.x), lin(color.y), lin(color.z), a], emissive: [lin(e.x), lin(e.y), lin(e.z)],
            metalness: pose.metalness, roughness: pose.roughness, unlit: m.unlit, doubleSided: m.doubleSided,
            texture: m.texture, mrMap: m.mrMap, normalMap: m.normalMap, normalScale: m.normalScale,
            emissiveMap: m.emissiveMap, aoMap: m.aoMap, alpha: m.alpha, cutoff: m.cutoff)
        return PrismSceneDraw(
            node: i, key: row.flat.key, name: row.named.flatMap { rows[$0].flat.name }, mesh: mesh,
            meshKey: row.meshKey, world: row.world, material: material,
            transparent: m.alpha == .blend && a < 1, instances: row.flat.instances.map { ($0.id, $0.count) },
            skinned: !row.jointNodes.isEmpty && row.joints.count == row.jointNodes.count,
            castShadows: row.flat.castShadows, receiveShadows: row.flat.receiveShadows, bounds: row.bounds)
    }

    /// What a tap at `(x, y)` (points from the element's top-left) hits in
    /// the current frame (`ScenePlayer::hit`).
    func hit(_ x: Float, _ y: Float, _ width: Float, _ height: Float, dark: Bool) -> PrismSceneTap {
        guard width > 0, height > 0 else { return PrismSceneTap() }
        let frame = self.frame(width, height, dark: dark)
        let (nx, ny) = (2 * x / width - 1, 1 - 2 * y / height)
        let near = PrismSceneMath.transformPoint(frame.inverseViewProj, [nx, ny, -1])
        let far = PrismSceneMath.transformPoint(frame.inverseViewProj, [nx, ny, 1])
        let dir = far - near
        // Nearest so far: (ray parameter, draw's node, instance).
        var best: (t: Float, node: Int, instance: Int?)?
        func closer(_ t: Float) -> Bool { best.map { t < $0.t } ?? true }
        for d in frame.draws {
            guard let t = PrismSceneMath.rayAabb(near, dir, d.bounds), closer(t) else { continue }
            let g = PrismSceneMeshes.shared.data(d.mesh, d.meshKey)
            let localBox = d.mesh.bounds
            let joints = d.skinned && g.joints.count == g.vertexCount * 4 ? rows[d.node].joints : nil
            // `m`: where the mesh is (its world, times an instance's matrix).
            func test(_ m: PrismMat4, _ instance: Int?) {
                if let joints {
                    // Skinned: its triangles in the current pose, in the world.
                    let placed = joints.map { m * $0 }
                    let p = (0..<g.vertexCount).map { prismSceneSkinnedPosition(g, $0, placed, m) }
                    var i = 0
                    while i + 3 <= g.indices.count {
                        let hit = PrismSceneMeshData.rayTriangle(near, dir, p[Int(g.indices[i])], p[Int(g.indices[i + 1])],
                                                                 p[Int(g.indices[i + 2])])
                        i += 3
                        if let t = hit, closer(t) { best = (t, d.node, instance) }
                    }
                    return
                }
                let inv = PrismSceneMath.invert(m)
                let o = PrismSceneMath.transformPoint(inv, near)
                let v = PrismSceneMath.transformDir(inv, dir)
                guard let t = PrismSceneMath.rayAabb(o, v, localBox), closer(t) else { return }
                var i = 0
                while i + 3 <= g.indices.count {
                    let hit = PrismSceneMeshData.rayTriangle(o, v, g.position(g.indices[i]), g.position(g.indices[i + 1]),
                                                             g.position(g.indices[i + 2]))
                    i += 3
                    if let t = hit, closer(t) { best = (t, d.node, instance) }
                }
            }
            if let inst = d.instances, let set = PrismSceneBlobs.shared.instances(inst.id) {
                for (k, m) in set.matrices.enumerated() { test(d.world * m, k) }
            } else {
                test(d.world, nil)
            }
        }
        guard let best else { return PrismSceneTap() }
        let row = rows[best.node]
        return PrismSceneTap(node: row.named.flatMap { rows[$0].flat.name }, part: row.part.flatMap { rows[$0].flat.label },
                             instance: best.instance, point: near + dir * best.t)
    }
}

/// Vertex `i` of a skinned mesh in the current pose: `Σ w_k · placed[j_k] ·
/// v`; by `world` alone if its weights are all zero.
private func prismSceneSkinnedPosition(_ g: PrismSceneMeshData, _ i: Int, _ placed: [PrismMat4], _ world: PrismMat4) -> PrismVec3 {
    let v = g.position(UInt32(i))
    var out = PrismVec3.zero
    var total: Float = 0
    for k in 0..<4 {
        let w = i * 4 + k < g.weights.count ? g.weights[i * 4 + k] : 0
        let j = Int(g.joints[i * 4 + k])
        guard j < placed.count else { continue }
        if w != 0 {
            out += PrismSceneMath.transformPoint(placed[j], v) * w
            total += w
        }
    }
    return total == 0 ? PrismSceneMath.transformPoint(world, v) : out
}

/// The shadow map fitted to the visible draws, seen along `light`
/// (`player::shadow_pass`).
private func prismSceneShadowPass(_ index: Int, _ light: PrismSceneFrameLight, _ draws: [PrismSceneDraw]) -> PrismSceneShadowPass? {
    guard let first = draws.first else { return nil }
    let union = draws.dropFirst().reduce(first.bounds) { $0.union($1.bounds) }
    let casters = draws.indices.filter { draws[$0].castShadows }
    if casters.isEmpty { return nil }
    let center = union.center
    var radius = simd_length(union.hi - union.lo) / 2
    // The radius in steps of 1/16 of its power of two, and the center on
    // whole texels: the map does not shimmer while the scene stays put.
    let step = pow(2, ceil(log2(max(radius, 1e-3))) - 4)
    radius = ceil(max(radius, 1e-3) / step) * step
    let size = Float(prismSceneShadowMapSize)
    let texel = 2 * radius / size
    let dir = light.direction
    let up: PrismVec3 = abs(dir.y) > 0.99 ? [0, 0, 1] : [0, 1, 0]
    let rotate = PrismSceneMath.lookAt(.zero, dir, up)
    let c = PrismSceneMath.transformPoint(rotate, center)
    func snap(_ v: Float) -> Float { floor(v / texel) * texel }
    let view = PrismSceneMath.translation([-snap(c.x), -snap(c.y), 0]) * rotate
    // One texel of margin for the snap; depth around the center (view −Z).
    let half = radius + texel
    let (near, far) = (-c.z - radius, -c.z + radius)
    let projection = PrismSceneMath.orthographic(2 * half, 1, near, far)
    return PrismSceneShadowPass(light: index, viewProj: projection * view, view: view, half: half, near: near, far: far,
                                mapSize: prismSceneShadowMapSize, bias: 1.5 / size, normalBias: 1.5 * texel,
                                casters: casters)
}

// MARK: Textures

/// Decoded texture images by source, shared by every scene. Rust loads
/// picture sources (`prism_scene_texture`; the watch host loads its own);
/// `p3d:<id>` images come from the blob store.
final class PrismSceneTextures {
    static let shared = PrismSceneTextures()
    private(set) var images: [String: PrismSceneImage] = [:]
    private let renderers = NSHashTable<PrismSceneRenderer>.weakObjects()

    func observe(_ r: PrismSceneRenderer) { renderers.add(r) }

    func add(_ src: String, _ image: PrismSceneImage) {
        images[src] = image
        for r in renderers.allObjects { r.texturesChanged() }
    }

    /// The image of `src`, once loaded.
    func image(_ src: String) -> PrismSceneImage? {
        if src.hasPrefix("p3d:") { return PrismSceneBlobs.shared.image(String(src.dropFirst(4))) }
        return images[src]
    }

    /// Every live scene renderer.
    var live: [PrismSceneRenderer] { renderers.allObjects }
}

/// A texture's image bytes, loaded by Rust (`bytes` null: it failed).
@_cdecl("prism_scene_texture")
public func prismSceneTexture(_ src: UnsafePointer<CChar>, _ bytes: UnsafePointer<UInt8>?, _ len: Int) {
    guard let bytes, let image = PrismSceneImage(data: Data(bytes: bytes, count: len)) else { return }
    PrismSceneTextures.shared.add(String(cString: src), image)
}

// MARK: SceneKit geometry

/// SceneKit sources of every mesh, built once per mesh key (and instance
/// set) and shared by every node and scene drawing it: each node gets its
/// own `SCNGeometry` (its own material) over the same sources, hence the
/// same GPU buffers.
final class PrismSceneGeometries {
    static let shared = PrismSceneGeometries()

    final class Built {
        let sources: [SCNGeometrySource]
        let element: SCNGeometryElement
        /// Joint weights and indices (skinned meshes).
        let bones: (weights: SCNGeometrySource, indices: SCNGeometrySource)?
        /// The largest joint index used.
        let maxJoint: Int

        init(sources: [SCNGeometrySource], element: SCNGeometryElement,
             bones: (weights: SCNGeometrySource, indices: SCNGeometrySource)?, maxJoint: Int) {
            self.sources = sources
            self.element = element
            self.bones = bones
            self.maxJoint = maxJoint
        }

        func geometry() -> SCNGeometry { SCNGeometry(sources: sources, elements: [element]) }
    }

    /// Instanced meshes merge their instances into one geometry up to this
    /// many vertices (one draw call); larger sets are drawn as clones.
    static let mergeLimit = 2_000_000

    private var meshes: [String: Built] = [:]
    private var merged: [String: Built] = [:]

    private static func source(_ a: [Float], _ semantic: SCNGeometrySource.Semantic, _ n: Int) -> SCNGeometrySource {
        SCNGeometrySource(data: a.withUnsafeBufferPointer { Data(buffer: $0) }, semantic: semantic,
                          vectorCount: a.count / n, usesFloatComponents: true, componentsPerVector: n,
                          bytesPerComponent: 4, dataOffset: 0, dataStride: n * 4)
    }

    private static func element(_ indices: [UInt32]) -> SCNGeometryElement {
        SCNGeometryElement(data: indices.withUnsafeBufferPointer { Data(buffer: $0) }, primitiveType: .triangles,
                           primitiveCount: indices.count / 3, bytesPerIndex: 4)
    }

    /// A mesh's sources (nil while it has no triangles).
    func built(_ mesh: PrismSceneMesh, _ key: String) -> Built? {
        if let b = meshes[key] { return b }
        let m = PrismSceneMeshes.shared.data(mesh, key)
        guard !m.indices.isEmpty else { return nil }
        var sources = [Self.source(m.positions, .vertex, 3), Self.source(m.normals, .normal, 3),
                       Self.source(m.uvs, .texcoord, 2)]
        if !m.colors.isEmpty { sources.append(Self.source(m.colors, .color, 4)) }
        var bones: (SCNGeometrySource, SCNGeometrySource)?
        var maxJoint = -1
        if m.joints.count == m.vertexCount * 4, m.weights.count == m.joints.count {
            let n = m.vertexCount
            let weights = Self.source(m.weights, .boneWeights, 4)
            let indices = SCNGeometrySource(data: m.joints.withUnsafeBufferPointer { Data(buffer: $0) },
                                            semantic: .boneIndices, vectorCount: n, usesFloatComponents: false,
                                            componentsPerVector: 4, bytesPerComponent: 2, dataOffset: 0, dataStride: 8)
            bones = (weights, indices)
            maxJoint = Int(m.joints.max() ?? 0)
        }
        let b = Built(sources: sources, element: Self.element(m.indices), bones: bones, maxJoint: maxJoint)
        meshes[key] = b
        return b
    }

    /// A mesh with every instance of a set baked in (`T · R · S` per
    /// instance, normals by its inverse transpose, instance colors as
    /// vertex colors): one geometry, one draw call. Nil when the mesh or the
    /// set is missing, or too big to merge.
    func merged(_ mesh: PrismSceneMesh, _ key: String, _ instances: String) -> Built? {
        let mkey = key + "|" + instances
        if let b = merged[mkey] { return b }
        let m = PrismSceneMeshes.shared.data(mesh, key)
        guard !m.indices.isEmpty, let set = PrismSceneBlobs.shared.instances(instances), !set.matrices.isEmpty,
              m.vertexCount * set.matrices.count <= Self.mergeLimit else { return nil }
        let n = m.vertexCount, c = set.matrices.count
        var pos = [Float](repeating: 0, count: n * c * 3)
        var nrm = [Float](repeating: 0, count: n * c * 3)
        var uv = [Float](repeating: 0, count: n * c * 2)
        let colored = !m.colors.isEmpty || !set.colors.isEmpty
        var col = [Float](repeating: 1, count: colored ? n * c * 4 : 0)
        var idx = [UInt32](repeating: 0, count: m.indices.count * c)
        for k in 0..<c {
            let w = set.matrices[k]
            let m3 = simd_float3x3(SIMD3(w.columns.0.x, w.columns.0.y, w.columns.0.z),
                                   SIMD3(w.columns.1.x, w.columns.1.y, w.columns.1.z),
                                   SIMD3(w.columns.2.x, w.columns.2.y, w.columns.2.z))
            let nm = m3.determinant != 0 ? m3.inverse.transpose : m3
            let tint = k < set.colors.count ? set.colors[k] : SIMD4<Float>(1, 1, 1, 1)
            for v in 0..<n {
                let o = (k * n + v)
                let p = PrismSceneMath.transformPoint(w, m.position(UInt32(v)))
                let q = PrismSceneMath.normalize(nm * PrismVec3(m.normals[v * 3], m.normals[v * 3 + 1], m.normals[v * 3 + 2]))
                pos[o * 3] = p.x; pos[o * 3 + 1] = p.y; pos[o * 3 + 2] = p.z
                nrm[o * 3] = q.x; nrm[o * 3 + 1] = q.y; nrm[o * 3 + 2] = q.z
                uv[o * 2] = m.uvs[v * 2]; uv[o * 2 + 1] = m.uvs[v * 2 + 1]
                if colored {
                    for j in 0..<4 { col[o * 4 + j] = (m.colors.isEmpty ? 1 : m.colors[v * 4 + j]) * tint[j] }
                }
            }
            let base = UInt32(k * n)
            for (i, x) in m.indices.enumerated() { idx[k * m.indices.count + i] = x + base }
        }
        var sources = [Self.source(pos, .vertex, 3), Self.source(nrm, .normal, 3), Self.source(uv, .texcoord, 2)]
        if colored { sources.append(Self.source(col, .color, 4)) }
        let b = Built(sources: sources, element: Self.element(idx), bones: nil, maxJoint: -1)
        merged[mkey] = b
        return b
    }

    func prune(keeping keys: Set<String>) {
        meshes = meshes.filter { keys.contains($0.key) }
        merged = merged.filter { keys.contains($0.key) }
    }
}

// MARK: SceneKit

func prismSceneColor(_ c: SIMD4<Float>) -> PrismSceneColor {
    #if os(macOS)
    NSColor(srgbRed: CGFloat(c.x), green: CGFloat(c.y), blue: CGFloat(c.z), alpha: CGFloat(c.w))
    #else
    UIColor(red: CGFloat(c.x), green: CGFloat(c.y), blue: CGFloat(c.z), alpha: CGFloat(c.w))
    #endif
}

/// The factors `shaders.rs` multiplies the textures with, and its alpha
/// modes (SceneKit multiplies the vertex colors in itself).
private let prismSceneSurface = """
#pragma arguments
float4 prismTint;
float prismMetal;
float prismRough;
float3 prismEmit;
float prismCutoff;
float prismOpaque;
#pragma body
_surface.diffuse *= prismTint;
_surface.metalness *= prismMetal;
_surface.roughness *= prismRough;
_surface.emission.rgb *= prismEmit;
if (prismCutoff > 0.0) {
    if (_surface.diffuse.a < prismCutoff) { discard_fragment(); }
    _surface.diffuse.a = 1.0;
} else if (prismOpaque > 0.5) {
    _surface.diffuse.a = 1.0;
}
"""

/// `shaders.rs`'s output: exposure, ACES (Narkowicz), then `pow(1/2.2)`.
/// The framebuffer encodes sRGB itself: it gets the linear value whose
/// sRGB code is the reference's. SceneKit's color is premultiplied here.
private let prismSceneTone = """
#pragma arguments
float prismExposure;
#pragma body
float a = _output.color.a;
float3 c = a > 0.0 ? _output.color.rgb / a : float3(0.0);
c *= prismExposure;
c = clamp((c * (2.51 * c + 0.03)) / (c * (2.43 * c + 0.59) + 0.14), 0.0, 1.0);
c = pow(c, float3(1.0 / 2.2));
c = select(pow((c + 0.055) / 1.055, float3(2.4)), c / 12.92, c <= 0.04045);
_output.color.rgb = c * a;
"""

/// A scene's SceneKit graph, moved by a `PrismScenePlayer`: every mesh is a
/// child of the root at its world matrix (the player resolves the
/// hierarchy, spins, transitions, clips and skins).
final class PrismSceneRenderer: NSObject {
    /// SceneKit's physically based lights are in lumens: 1000 lights a white
    /// surface white, like intensity 1 (measured).
    static let lumens: CGFloat = 1000

    let scene = SCNScene()
    let cameraNode = SCNNode()
    let player = PrismScenePlayer(now: PrismScenePlayer.clock())
    /// Asks the host to draw again (a texture arrived).
    var changed: (() -> Void)?
    /// The watch's SwiftUI view has no background color of its own: the
    /// scene paints it.
    var paintsBackground = false
    private(set) var width: Float = 0
    private(set) var height: Float = 0
    private(set) var dark = false

    private final class Entry {
        let node = SCNNode()
        let material = PrismSceneRenderer.material()
        /// What its geometry was built for (mesh, instance set, skin).
        var built: String?
        var look: Look?
        var world: PrismMat4?
        var bones: [SCNNode] = []
        var skinTick: UInt64 = 0
        var bounds: PrismAabb?
        var order = -1
    }

    /// What a material shows (applied when it changes).
    private struct Look: Equatable {
        var material: PrismSceneDrawMaterial
        var transparent: Bool
        /// Its images that are loaded.
        var loaded: [Bool]
        var exposure: Float
    }

    private struct LightsKey: Equatable {
        var lights: [PrismSceneLight]
        var dark: Bool
        var shadow: Int?
        var twin: Bool
    }

    private struct EnvKey: Equatable {
        var env: PrismSceneFrameEnv
        var exposure: Float
    }

    private var entries: [String: Entry] = [:]
    private let bonesRoot = SCNNode()
    private var lightNodes: [SCNNode] = []
    private var lightsKey: LightsKey?
    /// The shadow light's node and the camera it was given.
    private var shadowNode: SCNNode?
    private var shadowShown: (PrismMat4, Float, Float, Float)?
    private var envShown: EnvKey?
    private var envLoaded = false
    private var backgroundShown: AnyObject?

    override init() {
        super.init()
        cameraNode.camera = SCNCamera()
        scene.rootNode.addChildNode(cameraNode)
        scene.rootNode.addChildNode(bonesRoot)
        PrismSceneTextures.shared.observe(self)
    }

    var settings: PrismSceneSettings { player.settings }

    private static func material() -> SCNMaterial {
        let m = SCNMaterial()
        m.shaderModifiers = [.surface: prismSceneSurface, .fragment: prismSceneTone]
        return m
    }

    /// Shows an update.
    func apply(_ update: PrismSceneUpdate) {
        player.apply(update, now: PrismScenePlayer.clock())
        let keys = Set(player.rows.map(\.flat.key))
        for (key, e) in entries where !keys.contains(key) {
            Self.drop(e)
            entries[key] = nil
        }
        // The caches keep what some scene uses.
        var used = Set<String>()
        for r in PrismSceneTextures.shared.live { used.formUnion(r.currentUse()) }
        PrismSceneBlobs.shared.prune(keeping: used)
        PrismSceneMeshes.shared.prune(keeping: used)
        PrismSceneGeometries.shared.prune(keeping: used)
    }

    /// The blob ids and mesh keys it uses (the caches keep them).
    private func currentUse() -> Set<String> {
        var now = Set<String>()
        for row in player.rows {
            let f = row.flat
            if let mesh = f.mesh {
                now.insert(row.meshKey)
                if let i = f.instances { now.insert(row.meshKey + "|" + i.id) }
                if case let .blob(id, _) = mesh { now.insert(id) }
            }
            if let i = f.instances { now.insert(i.id) }
            if let p = f.play { now.insert(p.clip) }
            if let s = f.skin { now.insert(s.id) }
            for t in f.material.textures where t.hasPrefix("p3d:") { now.insert(String(t.dropFirst(4))) }
        }
        if let e = settings.env { now.insert(e.id) }
        return now
    }

    private static func drop(_ e: Entry) {
        e.node.removeFromParentNode()
        e.bones.forEach { $0.removeFromParentNode() }
    }

    func texturesChanged() {
        changed?()
    }

    /// The background color (nil: transparent).
    func background(dark: Bool) -> PrismSceneColor? {
        settings.background.map { prismSceneColor($0.resolve(dark)) }
    }

    /// Moves time to now and the SceneKit nodes with it; true while
    /// something animates (draw again on the next display frame).
    @discardableResult
    func tick(width: Float, height: Float, dark: Bool) -> Bool {
        self.width = width
        self.height = height
        self.dark = dark
        player.advance(PrismScenePlayer.clock())
        let frame = player.frame(width, height, dark: dark)
        SCNTransaction.begin()
        SCNTransaction.disableActions = true

        let c = settings.camera
        if let camera = cameraNode.camera {
            camera.zNear = Double(c.near)
            camera.zFar = Double(c.far)
            camera.projectionDirection = .vertical
            camera.wantsHDR = false
            if let h = frame.orbit.ortho {
                camera.usesOrthographicProjection = true
                camera.orthographicScale = Double(h / 2)
            } else {
                camera.usesOrthographicProjection = false
                camera.fieldOfView = CGFloat(frame.orbit.fov)
            }
        }
        // SceneKit cameras look down their -Z with +Y up, like the view matrix.
        cameraNode.simdTransform = frame.view.inverse

        showLights(frame)
        showEnvironment(frame)

        let tex = PrismSceneTextures.shared
        var shown = Set<String>()
        for (i, d) in frame.draws.enumerated() {
            let e: Entry
            if let found = entries[d.key] {
                e = found
            } else {
                e = Entry()
                entries[d.key] = e
                scene.rootNode.addChildNode(e.node)
            }
            build(e, d)
            place(e, d)
            e.node.isHidden = false
            e.node.castsShadow = d.castShadows
            // Not receiving shadows: lit by the shadow light's shadowless twin.
            e.node.categoryBitMask = d.receiveShadows ? 1 : 2
            // Transparent meshes after the opaque ones, back to front.
            let order = d.transparent ? 1000 + i : 0
            if e.order != order {
                e.order = order
                e.node.renderingOrder = order
                e.node.childNodes.forEach { $0.renderingOrder = order }
            }
            let m = d.material
            let loaded = [m.texture, m.mrMap, m.normalMap, m.emissiveMap, m.aoMap].map { $0.map { tex.image($0) != nil } ?? false }
            let look = Look(material: m, transparent: d.transparent, loaded: loaded, exposure: frame.exposure)
            if e.look != look {
                e.look = look
                Self.apply(look, to: e.material)
            }
            shown.insert(d.key)
        }
        for (key, e) in entries where !shown.contains(key) { e.node.isHidden = true }
        SCNTransaction.commit()
        return frame.animating
    }

    /// Gives an entry the geometry of its draw (again when its mesh,
    /// instances or skin changed; retried while a blob is missing).
    private func build(_ e: Entry, _ d: PrismSceneDraw) {
        let row = player.rows[d.node]
        let signature = "\(d.meshKey)|\(d.instances?.id ?? "")|\(d.skinned ? row.jointNodes.count : -1)"
        if e.built == signature { return }
        e.node.geometry = nil
        e.node.skinner = nil
        e.node.childNodes.forEach { $0.removeFromParentNode() }
        e.bones.forEach { $0.removeFromParentNode() }
        e.bones = []
        e.world = nil
        e.bounds = nil
        e.skinTick = 0
        e.order = -1
        let geometries = PrismSceneGeometries.shared
        if let inst = d.instances {
            if let b = geometries.merged(d.mesh, d.meshKey, inst.id) {
                let g = b.geometry()
                g.materials = [e.material]
                e.node.geometry = g
            } else if let b = geometries.built(d.mesh, d.meshKey), let set = PrismSceneBlobs.shared.instances(inst.id) {
                // Too many vertices to merge: one clone per instance.
                let g = b.geometry()
                g.materials = [e.material]
                for m in set.matrices {
                    let clone = SCNNode(geometry: g)
                    clone.simdTransform = m
                    e.node.addChildNode(clone)
                }
            } else {
                return
            }
        } else {
            guard let b = geometries.built(d.mesh, d.meshKey) else { return }
            let g = b.geometry()
            g.materials = [e.material]
            e.node.geometry = g
            if d.skinned, let bones = b.bones, b.maxJoint < row.jointNodes.count {
                // SceneKit skins on the GPU: each bone node is at the
                // player's `world(joint) · inverse_bind` (the node's own
                // transform places nothing, as in glTF).
                e.bones = row.jointNodes.map { _ in SCNNode() }
                e.bones.forEach(bonesRoot.addChildNode)
                let identity = NSValue(scnMatrix4: SCNMatrix4Identity)
                e.node.skinner = SCNSkinner(baseGeometry: g, bones: e.bones,
                                            boneInverseBindTransforms: e.bones.map { _ in identity },
                                            boneWeights: bones.weights, boneIndices: bones.indices)
            }
        }
        e.built = signature
    }

    /// Puts an entry where its draw is.
    private func place(_ e: Entry, _ d: PrismSceneDraw) {
        guard e.node.skinner != nil else {
            if e.world != d.world {
                e.world = d.world
                e.node.simdTransform = d.world
            }
            return
        }
        let row = player.rows[d.node]
        if e.skinTick != row.skinTick, row.placed.count == e.bones.count {
            e.skinTick = row.skinTick
            for (k, bone) in e.bones.enumerated() { bone.simdTransform = row.placed[k] }
        }
        if e.world != matrix_identity_float4x4 {
            e.world = matrix_identity_float4x4
            e.node.simdTransform = matrix_identity_float4x4
        }
        // Its box is the player's (the union of the mesh box under every
        // joint), for SceneKit's culling.
        if e.bounds != d.bounds, let g = e.node.geometry {
            e.bounds = d.bounds
            g.boundingBox = (SCNVector3(d.bounds.lo), SCNVector3(d.bounds.hi))
        }
    }

    /// What a tap at `(x, y)` points hits.
    func hit(x: Float, y: Float) -> PrismSceneTap {
        player.hit(x, y, width, height, dark: dark)
    }

    private func showLights(_ frame: PrismSceneFrame) {
        let twin = frame.shadow != nil && player.rows.contains { !$0.flat.receiveShadows && $0.flat.mesh != nil }
        let key = LightsKey(lights: settings.effectiveLights, dark: dark, shadow: frame.shadow?.light, twin: twin)
        if key != lightsKey {
            lightsKey = key
            rebuildLights(frame, twin: twin)
        }
        guard let s = frame.shadow, let node = shadowNode, let light = node.light else { return }
        if let (view, half, near, far) = shadowShown, view == s.view, half == s.half, near == s.near, far == s.far { return }
        shadowShown = (s.view, s.half, s.near, s.far)
        // The reference's light camera, moved back along its −Z so that its
        // near plane is just in front of it (SceneKit wants it positive).
        let depth = s.far - s.near
        let margin = max(depth * 0.01, 1e-3)
        let view = PrismSceneMath.translation([0, 0, s.near - margin]) * s.view
        node.simdTransform = view.inverse
        light.orthographicScale = CGFloat(s.half)
        light.zNear = CGFloat(margin)
        light.zFar = CGFloat(margin + depth)
    }

    private func rebuildLights(_ frame: PrismSceneFrame, twin: Bool) {
        lightNodes.forEach { $0.removeFromParentNode() }
        lightNodes = []
        shadowNode = nil
        shadowShown = nil
        func add(_ l: PrismSceneLight, _ direction: PrismVec3, shadow: Bool, category: Int?) {
            let light = SCNLight()
            let c = l.color.resolve(dark)
            light.color = prismSceneColor([c.x, c.y, c.z, 1])
            light.intensity = CGFloat(l.intensity) * Self.lumens
            let node = SCNNode()
            node.light = light
            switch l.kind {
            case .ambient: light.type = .ambient
            case .directional: light.type = .directional
            case .point: light.type = .omni
            case .spot:
                light.type = .spot
                let angle = CGFloat(max(l.angle, 1))
                light.spotOuterAngle = angle
                light.spotInnerAngle = angle * 0.75
            }
            if l.kind == .point || l.kind == .spot {
                node.simdPosition = l.position
                if l.range > 0 {
                    light.attenuationStartDistance = 0
                    light.attenuationEndDistance = CGFloat(l.range)
                    light.attenuationFalloffExponent = 2
                }
            }
            // A SceneKit light shines down its -Z.
            if l.kind == .directional || l.kind == .spot {
                node.simdOrientation = simd_quatf(from: [0, 0, -1], to: direction)
            }
            if let category { light.categoryBitMask = category }
            if shadow {
                light.castsShadow = true
                light.shadowMode = .forward
                light.shadowMapSize = CGSize(width: prismSceneShadowMapSize, height: prismSceneShadowMapSize)
                light.automaticallyAdjustsShadowProjection = false
                light.shadowCascadeCount = 1
                light.shadowColor = PrismSceneColor.black
                light.shadowSampleCount = 4
                light.shadowRadius = 1.5
                light.shadowBias = 1
                shadowNode = node
            }
            scene.rootNode.addChildNode(node)
            lightNodes.append(node)
        }
        for l in frame.ambient { add(l, .zero, shadow: false, category: nil) }
        for (i, l) in frame.lights.enumerated() {
            let casts = frame.shadow?.light == i
            add(l.light, l.direction, shadow: casts, category: casts ? 1 : nil)
            // Meshes that do not receive shadows are lit by a shadowless copy.
            if casts && twin { add(l.light, l.direction, shadow: false, category: 2) }
        }
    }

    private func showEnvironment(_ frame: PrismSceneFrame) {
        let key = frame.env.map { EnvKey(env: $0, exposure: frame.exposure) }
        if key != envShown || (key != nil && !envLoaded) {
            envShown = key
            envLoaded = false
            scene.lightingEnvironment.contents = nil
            if let env = frame.env, let map = PrismSceneBlobs.shared.env(env.id) {
                scene.lightingEnvironment.contents = map.lighting
                scene.lightingEnvironment.intensity = CGFloat(env.intensity)
                envLoaded = true
            }
        }
        var shown: AnyObject?
        if let env = frame.env, env.background, envLoaded, let map = PrismSceneBlobs.shared.env(env.id) {
            shown = map.background(env.intensity * frame.exposure)
        } else if paintsBackground {
            shown = background(dark: dark) ?? PrismSceneColor.clear
        }
        let same = shown == nil ? backgroundShown == nil
            : shown === backgroundShown || (shown as? NSObject)?.isEqual(backgroundShown) == true
        if !same {
            backgroundShown = shown
            scene.background.contents = shown
        }
    }

    private static func apply(_ look: Look, to m: SCNMaterial) {
        let d = look.material
        let tex = PrismSceneTextures.shared
        func image(_ src: String?) -> PrismSceneImage? { src.flatMap { tex.image($0) } }
        func map(_ p: SCNMaterialProperty, _ image: PrismSceneImage?, _ fallback: Any?) {
            p.contents = image ?? fallback
            guard image != nil else { return }
            p.wrapS = .repeat
            p.wrapT = .repeat
            p.minificationFilter = .linear
            p.magnificationFilter = .linear
            p.mipFilter = .linear
            p.maxAnisotropy = 8
        }
        m.lightingModel = d.unlit ? .constant : .physicallyBased
        if d.unlit {
            // Constant shading is `ambient × ambient lights + diffuse`: no
            // ambient, so the color shows as is.
            m.locksAmbientWithDiffuse = false
            m.ambient.contents = PrismSceneColor.black
        } else {
            m.locksAmbientWithDiffuse = true
        }
        map(m.diffuse, image(d.texture), PrismSceneColor.white)
        let mr = image(d.mrMap)
        map(m.metalness, mr, NSNumber(value: 1))
        m.metalness.textureComponents = mr != nil ? .blue : .all
        map(m.roughness, mr, NSNumber(value: 1))
        m.roughness.textureComponents = mr != nil ? .green : .all
        let normal = image(d.normalMap)
        map(m.normal, normal, nil)
        m.normal.intensity = CGFloat(d.normalScale)
        map(m.emission, image(d.emissiveMap), PrismSceneColor.white)
        let ao = image(d.aoMap)
        map(m.ambientOcclusion, ao, nil)
        m.ambientOcclusion.textureComponents = ao != nil ? .red : .all
        m.isDoubleSided = d.doubleSided
        // Blended below alpha 1: SceneKit's transparency (premultiplied
        // over what is behind), no depth writes.
        m.transparency = look.transparent ? CGFloat(d.color.w) : 1
        m.blendMode = .alpha
        m.writesToDepthBuffer = !look.transparent
        let a: Float = look.transparent ? 1 : d.color.w
        m.setValue(NSValue(scnVector4: SCNVector4(d.color.x, d.color.y, d.color.z, a)), forKey: "prismTint")
        m.setValue(NSNumber(value: d.metalness), forKey: "prismMetal")
        m.setValue(NSNumber(value: d.roughness), forKey: "prismRough")
        m.setValue(NSValue(scnVector3: SCNVector3(d.emissive.x, d.emissive.y, d.emissive.z)), forKey: "prismEmit")
        m.setValue(NSNumber(value: d.alpha == .mask ? max(d.cutoff, 1e-6) : 0), forKey: "prismCutoff")
        m.setValue(NSNumber(value: d.alpha == .opaque ? 1 : 0), forKey: "prismOpaque")
        m.setValue(NSNumber(value: look.exposure), forKey: "prismExposure")
    }
}

#if !os(watchOS)
/// The settings every SceneKit view of a scene shares: no default lights,
/// no camera control of its own, drawn when the scene changes only.
private func prismSceneConfigure(_ v: SCNView, _ r: PrismSceneRenderer) {
    v.scene = r.scene
    v.pointOfView = r.cameraNode
    v.autoenablesDefaultLighting = false
    v.allowsCameraControl = false
    v.rendersContinuously = false
    v.isPlaying = false
    v.antialiasingMode = .multisampling4X
    v.backgroundColor = .clear
    // SceneKit's frame statistics (fps, draw calls) over the scene, for profiling.
    if ProcessInfo.processInfo.environment["PRISM_SCENE_STATS"] != nil { v.showsStatistics = true }
}

/// Calls `fire` from a display link without the link retaining the view.
private final class PrismSceneLinkTarget: NSObject {
    let fire: () -> Void
    init(_ fire: @escaping () -> Void) { self.fire = fire }
    @objc func tick() { fire() }
}

// MARK: UIKit and AppKit views (C ABI for prism-ios and prism-macos)

/// Called with a tap's `SceneTapEvent` JSON.
public typealias PrismSceneTapped = @convention(c) (UInt64, UnsafePointer<CChar>) -> Void

/// A new scene view for node `id` (+1 retained: Rust owns it).
@_cdecl("prism_scene_create")
public func prismSceneCreate(_ id: UInt64, _ tapped: PrismSceneTapped) -> UnsafeMutableRawPointer {
    let view = MainActor.assumeIsolated { PrismSceneView(id: id, tapped: tapped) }
    return Unmanaged.passRetained(view).toOpaque()
}

/// Applies a scene update: `SceneUpdate` JSON, `len` bytes.
@_cdecl("prism_scene_set")
public func prismSceneSet(_ view: UnsafeMutableRawPointer, _ json: UnsafePointer<UInt8>, _ len: Int) {
    guard let update = PrismSceneUpdate(json: Data(bytes: json, count: len)) else { return }
    MainActor.assumeIsolated {
        Unmanaged<PrismSceneView>.fromOpaque(view).takeUnretainedValue().show(update)
    }
}
#endif

#if os(iOS) || os(tvOS) || os(visionOS)
final class PrismSceneView: UIView, UIGestureRecognizerDelegate {
    let renderer = PrismSceneRenderer()
    private let scn = SCNView(frame: .zero, options: nil)
    private let id: UInt64
    private let tapped: PrismSceneTapped
    private var link: CADisplayLink?
    private let pan = UIPanGestureRecognizer()
    #if !os(tvOS)
    private let pinch = UIPinchGestureRecognizer()
    private let tap = UITapGestureRecognizer()
    #endif
    private var panLast = CGPoint.zero

    init(id: UInt64, tapped: @escaping PrismSceneTapped) {
        self.id = id
        self.tapped = tapped
        super.init(frame: .zero)
        isOpaque = false
        backgroundColor = .clear
        prismSceneConfigure(scn, renderer)
        scn.isOpaque = false
        scn.isUserInteractionEnabled = false
        scn.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        addSubview(scn)
        renderer.changed = { [weak self] in self?.redraw() }

        pan.addTarget(self, action: #selector(onPan(_:)))
        #if !os(tvOS)
        pan.maximumNumberOfTouches = 2
        #endif
        var recognizers: [UIGestureRecognizer] = [pan]
        #if !os(tvOS)
        pinch.addTarget(self, action: #selector(onPinch(_:)))
        tap.addTarget(self, action: #selector(onTap(_:)))
        recognizers += [pinch, tap]
        #endif
        for g in recognizers {
            g.delegate = self
            // Touches still reach the view: they pause auto-rotation.
            g.cancelsTouchesInView = false
            g.isEnabled = false
            addGestureRecognizer(g)
        }
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (view: PrismSceneView, _: UITraitCollection) in
            view.redraw()
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    deinit { link?.invalidate() }

    /// Takes touches only when it uses them: other scenes leave scrolling
    /// and the element's own handlers alone.
    private var interactive: Bool {
        let o = renderer.settings.orbit
        #if os(tvOS)
        return o?.rotate ?? false
        #else
        return o != nil || renderer.settings.tap
        #endif
    }

    func show(_ update: PrismSceneUpdate) {
        renderer.apply(update)
        let o = renderer.settings.orbit
        pan.isEnabled = o.map { $0.rotate || $0.pan } ?? false
        #if !os(tvOS)
        pinch.isEnabled = o?.zoom ?? false
        tap.isEnabled = renderer.settings.tap
        #endif
        redraw()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        scn.frame = bounds
        redraw()
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        redraw()
    }

    override var isHidden: Bool {
        didSet { redraw() }
    }

    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? {
        let v = super.hitTest(point, with: event)
        return v === self && !interactive ? nil : v
    }

    #if os(tvOS)
    override var canBecomeFocused: Bool { interactive }
    #endif

    /// Draws the current frame; keeps a display link running while
    /// something animates (none otherwise: no work while idle or offscreen).
    func redraw() {
        guard window != nil, !isHidden, bounds.width > 0, bounds.height > 0 else {
            stopLink()
            return
        }
        let animating = renderer.tick(width: Float(bounds.width), height: Float(bounds.height),
                                      dark: traitCollection.userInterfaceStyle == .dark)
        scn.backgroundColor = renderer.background(dark: renderer.dark) ?? .clear
        if animating {
            if link == nil {
                let l = CADisplayLink(target: PrismSceneLinkTarget { [weak self] in self?.redraw() },
                                      selector: #selector(PrismSceneLinkTarget.tick))
                l.add(to: .main, forMode: .common)
                link = l
            }
        } else {
            stopLink()
        }
    }

    private func stopLink() {
        link?.invalidate()
        link = nil
    }

    // Auto-rotation pauses while a finger is down.
    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        super.touchesBegan(touches, with: event)
        renderer.player.pointerDown()
        redraw()
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        super.touchesEnded(touches, with: event)
        released(event)
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        super.touchesCancelled(touches, with: event)
        released(event)
    }

    private func released(_ event: UIEvent?) {
        let down = event?.touches(for: self)?.contains { $0.phase != .ended && $0.phase != .cancelled } ?? false
        if !down {
            renderer.player.pointerUp()
            redraw()
        }
    }

    /// One finger turns the camera, two pan it (if the orbit allows).
    @objc private func onPan(_ g: UIPanGestureRecognizer) {
        switch g.state {
        case .began, .changed:
            let t = g.translation(in: self)
            let (dx, dy) = (Float(t.x - panLast.x), Float(t.y - panLast.y))
            panLast = t
            if g.numberOfTouches >= 2, renderer.settings.orbit?.pan == true {
                renderer.player.pan(dx, dy, height: Float(bounds.height))
            } else {
                renderer.player.drag(dx, dy)
            }
            redraw()
        default:
            panLast = .zero
        }
    }

    #if !os(tvOS)
    @objc private func onPinch(_ g: UIPinchGestureRecognizer) {
        guard g.state == .began || g.state == .changed else { return }
        renderer.player.zoom(Float(g.scale))
        g.scale = 1
        redraw()
    }

    @objc private func onTap(_ g: UITapGestureRecognizer) {
        let p = g.location(in: self)
        renderer.hit(x: Float(p.x), y: Float(p.y)).json.withCString { tapped(id, $0) }
    }
    #endif

    override func gestureRecognizerShouldBegin(_ g: UIGestureRecognizer) -> Bool {
        let rotate = renderer.settings.orbit?.rotate == true
        // One finger only turns: without rotation it is left to scroll views.
        if g === pan { return rotate || pan.numberOfTouches >= 2 }
        // A drag on a turning scene turns it rather than scroll the page.
        if rotate, g is UIPanGestureRecognizer, g.view !== self { return false }
        return super.gestureRecognizerShouldBegin(g)
    }

    func gestureRecognizer(_ g: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
        other.view === self
    }
}
#elseif os(macOS)
final class PrismSceneView: NSView {
    let renderer = PrismSceneRenderer()
    private let scn = SCNView(frame: .zero, options: nil)
    private let id: UInt64
    private let tapped: PrismSceneTapped
    private var link: CADisplayLink?
    /// The press under way: where it started and how far it moved (points).
    private var press: (start: NSPoint, last: NSPoint, moved: CGFloat, pan: Bool)?

    init(id: UInt64, tapped: @escaping PrismSceneTapped) {
        self.id = id
        self.tapped = tapped
        super.init(frame: .zero)
        wantsLayer = true
        prismSceneConfigure(scn, renderer)
        scn.wantsLayer = true
        scn.layer?.isOpaque = false
        scn.autoresizingMask = [.width, .height]
        addSubview(scn)
        renderer.changed = { [weak self] in self?.redraw() }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    deinit { link?.invalidate() }

    override var isFlipped: Bool { true }

    private var interactive: Bool { renderer.settings.orbit != nil || renderer.settings.tap }

    func show(_ update: PrismSceneUpdate) {
        renderer.apply(update)
        redraw()
    }

    /// Clicks reach the scene only when it uses them.
    override func hitTest(_ point: NSPoint) -> NSView? {
        interactive && !isHidden && frame.contains(point) ? self : nil
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func setFrameSize(_ size: NSSize) {
        super.setFrameSize(size)
        scn.frame = bounds
        redraw()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        redraw()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        redraw()
    }

    override func viewDidHide() {
        super.viewDidHide()
        redraw()
    }

    override func viewDidUnhide() {
        super.viewDidUnhide()
        redraw()
    }

    /// Draws the current frame; keeps a display link running while
    /// something animates (none otherwise: no work while idle or offscreen).
    func redraw() {
        guard window != nil, !isHiddenOrHasHiddenAncestor, bounds.width > 0, bounds.height > 0 else {
            stopLink()
            return
        }
        let dark = effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let animating = renderer.tick(width: Float(bounds.width), height: Float(bounds.height), dark: dark)
        scn.backgroundColor = renderer.background(dark: dark) ?? .clear
        if animating {
            if link == nil {
                let l = displayLink(target: PrismSceneLinkTarget { [weak self] in self?.redraw() },
                                    selector: #selector(PrismSceneLinkTarget.tick))
                l.add(to: .main, forMode: .common)
                link = l
            }
        } else {
            stopLink()
        }
    }

    private func stopLink() {
        link?.invalidate()
        link = nil
    }

    private func location(_ event: NSEvent) -> NSPoint { convert(event.locationInWindow, from: nil) }

    private func down(_ event: NSEvent, pan: Bool) {
        let p = location(event)
        press = (p, p, 0, pan)
        renderer.player.pointerDown()
        redraw()
    }

    /// Left drag turns the camera; right or Shift drag pans it.
    private func dragged(_ event: NSEvent) {
        guard var p = press else { return }
        let at = location(event)
        let (dx, dy) = (at.x - p.last.x, at.y - p.last.y)
        p.last = at
        p.moved = max(p.moved, hypot(at.x - p.start.x, at.y - p.start.y))
        press = p
        if p.pan || event.modifierFlags.contains(.shift) {
            renderer.player.pan(Float(dx), Float(dy), height: Float(bounds.height))
        } else {
            renderer.player.drag(Float(dx), Float(dy))
        }
        redraw()
    }

    private func up(_ event: NSEvent) {
        guard let p = press else { return }
        press = nil
        renderer.player.pointerUp()
        // A click that did not move is a tap.
        if p.moved < CGFloat(PrismScenePlayer.tapSlop), renderer.settings.tap {
            let at = location(event)
            renderer.hit(x: Float(at.x), y: Float(at.y)).json.withCString { tapped(id, $0) }
        }
        redraw()
    }

    override func mouseDown(with event: NSEvent) { down(event, pan: false) }
    override func mouseDragged(with event: NSEvent) { dragged(event) }
    override func mouseUp(with event: NSEvent) { up(event) }
    override func rightMouseDown(with event: NSEvent) { down(event, pan: true) }
    override func rightMouseDragged(with event: NSEvent) { dragged(event) }
    override func rightMouseUp(with event: NSEvent) { up(event) }

    /// Scrolling zooms (and scrolls the page when the scene does not zoom).
    override func scrollWheel(with event: NSEvent) {
        guard renderer.settings.orbit?.zoom == true else {
            super.scrollWheel(with: event)
            return
        }
        // Points on trackpads; a wheel's lines are about 40 points each.
        // Scrolling down (content up) moves away, as on the web.
        let dy = event.hasPreciseScrollingDeltas ? event.scrollingDeltaY : event.scrollingDeltaY * 40
        renderer.player.zoom(exp(Float(dy) * 0.002))
        redraw()
    }

    override func magnify(with event: NSEvent) {
        guard renderer.settings.orbit?.zoom == true else {
            super.magnify(with: event)
            return
        }
        renderer.player.zoom(Float(1 + event.magnification))
        redraw()
    }
}
#endif
