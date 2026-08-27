// CandidatePanel.swift — 候选窗（NSPanel + NSVisualEffectView）
//
// 皮肤材质映射（与 hufu-skin 的 Material 枚举一致）：
//   solid      → opaque 材质 + skin.back_color
//   translucent → behindWindowMenu 模糊 + 半透明 back_color
//   frosted    → behindWindow（磨砂/Acrylic 近似）+ 半透明 back_color
//   glass      → fullScreenUI 玻璃 + 边框高光 hairline

import Cocoa

struct CandSkin {
    var back: NSColor = NSColor(calibratedWhite: 0.12, alpha: 0.94)
    var border: NSColor = NSColor(calibratedWhite: 1.0, alpha: 0.15)
    var text: NSColor = .white
    var label: NSColor = NSColor(calibratedRed: 1.0, green: 0.84, blue: 0.37, alpha: 1)
    var comment: NSColor = NSColor(calibratedWhite: 0.62, alpha: 1)
    var hiliteBack: NSColor = NSColor(calibratedWhite: 0.24, alpha: 1)
    var font: NSFont = .systemFont(ofSize: 15)
    /// solid / translucent / frosted / glass
    var material: String = "frosted"

    static let shared = CandSkin()

    /// 从引擎 skin JSON 解析（"#RRGGBBAA"）。
    static func load() -> CandSkin {
        guard let resp = EngineClient.shared.call("skin"),
              let skin = resp["skin"] as? [String: Any],
              let colors = skin["colors"] as? [String: Any] else { return shared }
        var s = shared
        for (k, v) in colors {
            guard let hex = v as? String, let c = NSColor(hex: hex) else { continue }
            switch k {
            case "back_color": s.back = c
            case "border_color": s.border = c
            case "text_color": s.text = c
            case "hilited_candidate_label_color": s.label = c
            case "comment_text_color": s.comment = c
            case "hilited_candidate_back_color": s.hiliteBack = c
            default: break
            }
        }
        if let m = skin["material"] as? String { s.material = m }
        return s
    }
}

extension NSColor {
    convenience init?(hex: String) {
        var s = hex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6 || s.count == 8,
              let v = UInt64(s, radix: 16) else { return nil }
        let r = CGFloat((v >> 24) & 0xFF) / 255
        let g = CGFloat((v >> 16) & 0xFF) / 255
        let b = CGFloat((v >> 8) & 0xFF) / 255
        let a = s.count == 8 ? CGFloat(v & 0xFF) / 255 : 1
        self.init(calibratedRed: r, green: g, blue: b, alpha: a)
    }
}

final class CandidatePanel: NSPanel {
    private var effect: NSVisualEffectView!
    private var stack: NSStackView!
    private let skin: CandSkin
    private let codeField = NSTextField(labelWithString: "")

    init() {
        skin = CandSkin.load()
        let style: NSWindow.StyleMask = [.borderless, .nonactivatingPanel]
        super.init(contentRect: NSRect(x: 0, y: 0, width: 300, height: 120),
                   styleMask: style, backing: .buffered, defer: false)
        isFloatingPanel = true
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        hasShadow = true

        effect = NSVisualEffectView(frame: NSRect(x: 0, y: 0, width: 300, height: 120))
        effect.blendingMode = .behindWindow
        effect.material = {
            switch skin.material {
            case "solid": return .underWindowBackground
            case "translucent": return .menu
            case "glass": return .fullScreenUI
            default: return .underPageBackground // frosted
            }
        }()
        effect.state = .active
        effect.alphaValue = skin.material == "solid" ? 0 : 0.55
        contentView = effect

        stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.edgeInsets = NSEdgeInsets(top: 6, left: 10, bottom: 6, right: 10)
        stack.translatesAutoresizingMaskIntoConstraints = false
        effect.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: effect.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: effect.trailingAnchor),
            stack.topAnchor.constraint(equalTo: effect.topAnchor),
            stack.bottomAnchor.constraint(equalTo: effect.bottomAnchor),
        ])

        codeField.font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        codeField.textColor = skin.text
        stack.addArrangedSubview(codeField)
    }

    func update(cands: [(String, String)], raw: String) {
        // 清空旧视图
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        codeField.stringValue = raw
        stack.addArrangedSubview(codeField)
        for (i, (text, comment)) in cands.prefix(9).enumerated() {
            let row = NSStackView()
            row.orientation = .horizontal
            row.spacing = 6
            let lbl = NSTextField(labelWithString: "\(i + 1).")
            lbl.font = skin.font
            lbl.textColor = i == 0 ? skin.label : skin.text
            let body = NSTextField(labelWithString: text)
            body.font = skin.font
            body.textColor = i == 0 ? skin.label : skin.text
            row.addArrangedSubview(lbl)
            row.addArrangedSubview(body)
            if !comment.isEmpty {
                let cm = NSTextField(labelWithString: comment)
                cm.font = skin.font.withSize(skin.font.pointSize - 2)
                cm.textColor = skin.comment
                row.addArrangedSubview(cm)
            }
            stack.addArrangedSubview(row)
        }
    }

    func showBelow(_ caret: CGRect) {
        if let scr = NSScreen.main {
            let pos = NSRect(x: caret.minX,
                             y: scr.frame.maxY - caret.minY - caret.height - 180,
                             width: 320, height: stack.fittingSize.height + 24)
            setFrame(pos.display(), display: true)
        }
        orderFrontRegardless()
    }

    func hide() {
        orderOut(nil)
    }
}

extension NSRect {
    /// 保证不出屏幕右缘。
    func display() -> NSRect {
        guard let scr = NSScreen.main else { return self }
        if maxX > scr.frame.maxX {
            return NSRect(x: scr.frame.maxX - width, y: minY, width: width, height: height)
        }
        return self
    }
}
