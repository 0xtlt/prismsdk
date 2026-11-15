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

    func image(_ n: [String: Any]) -> UIImage? {
        (n["icon"] as? String).flatMap { UIImage(systemName: $0) }
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
                                image: image(n) ?? UIImage(systemName: "square")!) { _ in
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
#else
@_cdecl("prism_car_set")
public func prismCarSet(_ json: UnsafePointer<CChar>) {} // No CarPlay here.
#endif
