// CarPlay: the app's car templates (prism-core `car`, sent as JSON) as
// CPListTemplate, CPGridTemplate, CPInformationTemplate and CPTabBarTemplate.
// Selections and Back go to Rust; Rust sends the new stack, which is applied
// in place where the template kind is unchanged (no flashing).
import Foundation
#if os(iOS) && canImport(CarPlay)
import CarPlay
import UIKit

@_silgen_name("prism_car_connected")
func prismRustCarConnected(_ connected: Bool)
@_silgen_name("prism_car_select")
func prismRustCarSelect(_ id: UInt64)
@_silgen_name("prism_car_back")
func prismRustCarBack(_ depth: UInt64)

/// The CarPlay scene's delegate (Info.plist: CPTemplateApplicationSceneSessionRoleApplication).
@objc(PrismCarPlaySceneDelegate)
public final class PrismCarPlaySceneDelegate: UIResponder, CPTemplateApplicationSceneDelegate, CPInterfaceControllerDelegate {
    static var current: PrismCarPlaySceneDelegate?
    var controller: CPInterfaceController?
    /// The stack shown, by kind (`list`, `grid`, `pane`, `tabs`), and its templates.
    var kinds: [String] = []
    /// While Prism changes the stack itself, Back is not reported.
    var syncing = false

    public func templateApplicationScene(_ scene: CPTemplateApplicationScene,
                                         didConnect interfaceController: CPInterfaceController) {
        controller = interfaceController
        interfaceController.delegate = self
        PrismCarPlaySceneDelegate.current = self
        // A placeholder until Rust answers (CarPlay needs a root right away).
        interfaceController.setRootTemplate(CPListTemplate(title: "", sections: []), animated: false, completion: nil)
        kinds = []
        prismRustCarConnected(true)
        if let json = pendingCarJSON { apply(json) }
    }

    public func templateApplicationScene(_ scene: CPTemplateApplicationScene,
                                         didDisconnectInterfaceController interfaceController: CPInterfaceController) {
        controller = nil
        kinds = []
        PrismCarPlaySceneDelegate.current = nil
        prismRustCarConnected(false)
    }

    /// The driver went back: tell Rust how deep the stack now is.
    public func templateDidDisappear(_ aTemplate: CPTemplate, animated: Bool) {
        guard !syncing, let controller else { return }
        let depth = controller.templates.count
        if depth < kinds.count {
            kinds.removeLast(kinds.count - depth)
            prismRustCarBack(UInt64(max(depth - 1, 0)))
        }
    }

    func apply(_ json: String) {
        guard let controller,
              let data = json.data(using: .utf8),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let stack = root["stack"] as? [[String: Any]], !stack.isEmpty else { return }
        syncing = true
        defer { syncing = false }
        let shown = controller.templates
        for (i, node) in stack.enumerated() {
            let kind = node["t"] as? String ?? "list"
            if i < shown.count, i < kinds.count, kinds[i] == kind, update(shown[i], node) {
                continue
            }
            // A new kind at this depth: replace it and everything above.
            let template = build(node)
            if i == 0 {
                controller.setRootTemplate(template, animated: false, completion: nil)
            } else {
                while controller.templates.count > i { controller.popTemplate(animated: false, completion: nil) }
                controller.pushTemplate(template, animated: true, completion: nil)
            }
            kinds = Array(kinds.prefix(i)) + [kind]
        }
        if controller.templates.count > stack.count {
            if stack.count == 1 {
                controller.popToRootTemplate(animated: true, completion: nil)
            } else {
                while controller.templates.count > stack.count { controller.popTemplate(animated: true, completion: nil) }
            }
        }
        kinds = stack.map { $0["t"] as? String ?? "list" }
    }

    // MARK: Building

    func image(_ n: [String: Any], size: CGSize = CPListItem.maximumImageSize) -> UIImage? {
        if let drawing = n["drawing"] as? String { return prismCarDrawing(drawing, size: size) }
        return (n["icon"] as? String).flatMap { UIImage(systemName: $0) }
    }

    func select(_ n: [String: Any]) -> UInt64? {
        (n["id"] as? NSNumber)?.uint64Value
    }

    func sections(_ node: [String: Any]) -> [CPListSection] {
        ((node["sections"] as? [[String: Any]]) ?? []).map { s in
            let items: [CPListItem] = ((s["items"] as? [[String: Any]]) ?? []).map { n in
                let item = CPListItem(text: n["title"] as? String, detailText: n["detail"] as? String, image: image(n))
                if (n["browsable"] as? Bool) == true { item.accessoryType = .disclosureIndicator }
                if let id = select(n) {
                    item.handler = { _, done in prismRustCarSelect(id); done() }
                }
                return item
            }
            return CPListSection(items: items, header: s["header"] as? String, sectionIndexTitle: nil)
        }
    }

    func gridButtons(_ node: [String: Any]) -> [CPGridButton] {
        ((node["buttons"] as? [[String: Any]]) ?? []).map { n in
            let id = select(n)
            return CPGridButton(titleVariants: [n["title"] as? String ?? ""],
                                image: image(n, size: CGSize(width: 80, height: 80)) ?? UIImage(systemName: "square")!) { _ in
                if let id { prismRustCarSelect(id) }
            }
        }
    }

    func paneItems(_ node: [String: Any]) -> [CPInformationItem] {
        ((node["rows"] as? [[String]]) ?? []).map { CPInformationItem(title: $0.first, detail: $0.count > 1 ? $0[1] : nil) }
    }

    func paneActions(_ node: [String: Any]) -> [CPTextButton] {
        ((node["actions"] as? [[String: Any]]) ?? []).map { n in
            let style: CPTextButtonStyle = switch n["style"] as? String {
            case "primary": .confirm
            case "destructive": .cancel
            default: .normal
            }
            let id = select(n)
            return CPTextButton(title: n["title"] as? String ?? "", textStyle: style) { _ in
                if let id { prismRustCarSelect(id) }
            }
        }
    }

    func build(_ node: [String: Any]) -> CPTemplate {
        let title = node["title"] as? String ?? ""
        switch node["t"] as? String {
        case "grid":
            return CPGridTemplate(title: title, gridButtons: gridButtons(node))
        case "pane":
            return CPInformationTemplate(title: title, layout: .leading, items: paneItems(node), actions: paneActions(node))
        case "tabs":
            let tabs: [CPTemplate] = ((node["tabs"] as? [[String: Any]]) ?? []).map { tab in
                let t = build(tab["template"] as? [String: Any] ?? [:])
                t.tabTitle = tab["title"] as? String
                t.tabImage = image(tab)
                return t
            }
            return CPTabBarTemplate(templates: tabs)
        default:
            return CPListTemplate(title: title, sections: sections(node))
        }
    }

    /// Updates a template of the same kind in place; false to rebuild it.
    func update(_ template: CPTemplate, _ node: [String: Any]) -> Bool {
        switch (template, node["t"] as? String) {
        case (let list as CPListTemplate, "list"):
            guard list.title == node["title"] as? String else { return false }
            list.updateSections(sections(node))
        case (let grid as CPGridTemplate, "grid"):
            guard grid.title == node["title"] as? String else { return false }
            grid.updateGridButtons(gridButtons(node))
        case (let pane as CPInformationTemplate, "pane"):
            guard pane.title == node["title"] as? String else { return false }
            pane.items = paneItems(node)
            pane.actions = paneActions(node)
        case (let tabs as CPTabBarTemplate, "tabs"):
            let built = ((node["tabs"] as? [[String: Any]]) ?? []).map { tab -> CPTemplate in
                let t = build(tab["template"] as? [String: Any] ?? [:])
                t.tabTitle = tab["title"] as? String
                t.tabImage = image(tab)
                return t
            }
            tabs.updateTemplates(built)
        default:
            return false
        }
        return true
    }
}

/// The last stack, kept until CarPlay connects.
private var pendingCarJSON: String?

@_cdecl("prism_car_set")
public func prismCarSet(_ json: UnsafePointer<CChar>) {
    let json = String(cString: json)
    pendingCarJSON = json
    PrismCarPlaySceneDelegate.current?.apply(json)
}

// MARK: Charts in image slots

/// A prism-core drawing (compact text: `i<x>,<y>`, then per path paint
/// tokens, `|`, segments in fractions of the frame; see PrismRenderer.swift)
/// as a `UIImage` of `size` points, with light and dark variants (CarPlay
/// switches between day and night).
func prismCarDrawing(_ text: String, size: CGSize) -> UIImage {
    func render(dark: Bool) -> UIImage {
        let format = UIGraphicsImageRendererFormat()
        format.scale = 3
        return UIGraphicsImageRenderer(size: size, format: format).image { ctx in
            prismDrawPaths(text, in: CGRect(origin: .zero, size: size), dark: dark, ctx.cgContext)
        }
    }
    let light = render(dark: false)
    guard text.contains("/#") else { return light }
    let asset = UIImageAsset()
    asset.register(light, with: UITraitCollection(userInterfaceStyle: .light))
    asset.register(render(dark: true), with: UITraitCollection(userInterfaceStyle: .dark))
    return asset.image(with: UITraitCollection.current)
}

private func prismCarColor(_ spec: String, dark: Bool) -> UIColor? {
    let parts = spec.split(separator: "/")
    guard !parts.isEmpty else { return nil }
    var s = String(parts[dark && parts.count == 2 ? 1 : 0]).replacingOccurrences(of: "#", with: "")
    if s.count == 6 { s += "FF" }
    guard s.count == 8, let v = UInt64(s, radix: 16) else { return nil }
    return UIColor(red: CGFloat((v >> 24) & 0xFF) / 255, green: CGFloat((v >> 16) & 0xFF) / 255,
                   blue: CGFloat((v >> 8) & 0xFF) / 255, alpha: CGFloat(v & 0xFF) / 255)
}

/// Strokes and fills each path of `text` in `rect` (like prism-core `Drawing::resolve`).
func prismDrawPaths(_ text: String, in rect: CGRect, dark: Bool, _ cg: CGContext) {
    var (ix, iy) = (0.0, 0.0)
    for line in text.split(separator: "\n") {
        if line.hasPrefix("i") {
            let v = line.dropFirst().split(separator: ",").compactMap { Double($0) }
            (ix, iy) = (v.first ?? 0, v.count > 1 ? v[1] : 0)
            continue
        }
        guard let bar = line.firstIndex(of: "|") else { continue }
        var fill: UIColor?, stroke: UIColor?, width = 1.0, round = false, dash: [CGFloat] = []
        for token in line[..<bar].split(separator: " ") {
            let value = String(token.dropFirst())
            switch token.first {
            case "f": fill = prismCarColor(value, dark: dark)
            case "s": stroke = prismCarColor(value, dark: dark)
            case "w": width = Double(value) ?? 1
            case "r": round = true
            case "d": dash = value.split(separator: ",").compactMap { Double($0) }.map { CGFloat($0) }
            default: break
            }
        }
        let iw = max(0, rect.width - 2 * ix), ih = max(0, rect.height - 2 * iy), side = min(iw, ih)
        func pt(_ x: Double, _ y: Double) -> CGPoint { CGPoint(x: rect.minX + ix + x * iw, y: rect.minY + iy + y * ih) }
        let path = CGMutablePath()
        var open = false
        let t = line[line.index(after: bar)...].split(separator: " ")
        var i = 0
        func take(_ n: Int) -> [Double]? {
            guard i + n <= t.count else { return nil }
            let v = t[i..<i + n].compactMap { Double($0) }
            i += n
            return v.count == n ? v : nil
        }
        parse: while i < t.count {
            let cmd = t[i]
            i += 1
            switch cmd {
            case "M", "L":
                guard let v = take(2) else { break parse }
                if cmd == "L" && open { path.addLine(to: pt(v[0], v[1])) } else { path.move(to: pt(v[0], v[1])) }
                open = true
            case "C":
                guard let v = take(6) else { break parse }
                if !open { path.move(to: pt(v[0], v[1])) }
                path.addCurve(to: pt(v[4], v[5]), control1: pt(v[0], v[1]), control2: pt(v[2], v[3]))
                open = true
            case "A":
                guard let v = take(5) else { break parse }
                let c = pt(v[0], v[1]), r = v[2] * side
                func at(_ deg: Double) -> CGPoint {
                    CGPoint(x: c.x + r * sin(deg * .pi / 180), y: c.y - r * cos(deg * .pi / 180))
                }
                if open { path.addLine(to: at(v[3])) } else { path.move(to: at(v[3])) }
                open = true
                let sweep = min(max(v[4] - v[3], -360), 360)
                let n = max(1, Int((abs(sweep) / 90).rounded(.up)))
                let step = sweep / Double(n), k = 4 / 3 * tan(step * .pi / 720) * r
                for j in 0..<n {
                    let a0 = v[3] + step * Double(j), a1 = a0 + step
                    let p0 = at(a0), p1 = at(a1), r0 = a0 * .pi / 180, r1 = a1 * .pi / 180
                    path.addCurve(to: p1, control1: CGPoint(x: p0.x + k * cos(r0), y: p0.y + k * sin(r0)),
                                  control2: CGPoint(x: p1.x - k * cos(r1), y: p1.y - k * sin(r1)))
                }
            case "Z":
                path.closeSubpath()
                open = false
            default: break parse
            }
        }
        if let fill {
            cg.addPath(path)
            cg.setFillColor(fill.cgColor)
            cg.fillPath()
        }
        if let stroke {
            cg.addPath(path)
            cg.setStrokeColor(stroke.cgColor)
            cg.setLineWidth(width)
            cg.setLineCap(round ? .round : .butt)
            cg.setLineJoin(round ? .round : .miter)
            cg.setLineDash(phase: 0, lengths: dash.count == 2 ? dash : [])
            cg.strokePath()
        }
    }
}
#else
@_cdecl("prism_car_set")
public func prismCarSet(_ json: UnsafePointer<CChar>) {} // No CarPlay here.
#endif
