// HuFuInputController.swift — HuFu 虎符输入法 macOS IMK 前端
//
// 架构同 Windows 前端：本输入法是薄壳，按键经 Unix domain socket
// （$XDG_RUNTIME_DIR/hufu-ime.sock 或 /tmp/hufu-ime.sock）发给 hufu-server
// 引擎进程，回包 {consumed, commit, state} 驱动组段（marked text）与候选窗。
//
// 帧协议：4 字节小端长度 + JSON（与 Windows 命名管道一致）。

import Cocoa
import InputMethodKit

/// 引擎 IPC 客户端（每次请求一帧）。
final class EngineClient {
    static let shared = EngineClient()
    private var path: String {
        if let xdg = ProcessInfo.processInfo.environment["XDG_RUNTIME_DIR"] {
            return xdg + "/hufu-ime.sock"
        }
        return "/tmp/hufu-ime.sock"
    }

    func call(_ op: String, key: String? = nil, shift: Bool = false,
              ctrl: Bool = false, alt: Bool = false) -> [String: Any]? {
        var req: [String: Any] = ["op": op]
        if let key = key {
            req["key"] = key
            req["modifiers"] = ["shift": shift, "ctrl": ctrl, "alt": alt]
        }
        guard let body = try? JSONSerialization.data(withJSONObject: req) else { return nil }
        let sock = AF_UNIX.socket()
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let p = path.utf8CString
        withUnsafeBytes(of: p) { src in
            withUnsafeMutableBytes(of: &addr.sun_path.0) { dst in
                let n = min(src.count, MemoryLayout.size(ofValue: addr.sun_path))
                dst.copyMemory(from: UnsafeRawBufferPointer(rebasing: src.prefix(n)))
            }
        }
        let ok = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                connect(sock, sa, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard ok == 0 else { close(sock); return nil }
        defer { close(sock) }

        var frame = UInt32(body.count).littleEndian
        var sent = 0
        withUnsafeBytes(of: &frame) { _ = nil }
        // 写帧头+帧体
        var head = withUnsafeBytes(of: frame) { Array($0) }
        head.append(contentsOf: body)
        while sent < head.count {
            let n = head.withUnsafeBufferPointer { buf in
                write(sock, buf.baseAddress! + sent, head.count - sent)
            }
            if n <= 0 { return nil }
            sent += n
        }
        // 读响应
        var lenBuf = [UInt8](repeating: 0, count: 4)
        var got = 0
        while got < 4 {
            let n = read(sock, &lenBuf, 4 - got)
            if n <= 0 { return nil }
            got += n
        }
        let len = Int(lenBuf[0]) | (Int(lenBuf[1]) << 8) | (Int(lenBuf[2]) << 16) | (Int(lenBuf[3]) << 24)
        guard len > 0, len < 1 << 20 else { return nil }
        var resp = [UInt8](repeating: 0, count: len)
        got = 0
        while got < len {
            let n = read(sock, &resp, len - got)
            if n <= 0 { return nil }
            got += n
        }
        return (try? JSONSerialization.jsonObject(with: resp)) as? [String: Any]
    }
}

/// 主控制器：键盘事件 → 引擎 → 组段/上屏 + 候选窗。
@objc(HuFuInputController)
class HuFuInputController: IMKInputController {
    var panel: CandidatePanel?

    // MARK: - 键盘事件

    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        guard let event = event else { return false }

        // 修饰键单独抬起：交给引擎（Shift 切换中英）
        var keyName: String? = nil
        var shift = false, ctrl = false, alt = false
        shift = event.modifierFlags.contains(.shift)
        ctrl = event.modifierFlags.contains(.control)
        alt = event.modifierFlags.contains(.option)

        switch Int(event.keyCode) {
        case 36: keyName = "enter"
        case 48: keyName = "tab"
        case 49: keyName = "space"
        case 51: keyName = "backspace"
        case 53: keyName = "escape"
        case 117: keyName = "delete"
        case 123: keyName = "left"
        case 124: keyName = "right"
        case 125: keyName = "down"
        case 126: keyName = "up"
        case 116: keyName = "pageup"
        case 121: keyName = "pagedown"
        default:
            if let ch = event.charactersIgnoringModifiers, !ch.isEmpty {
                let c = ch[ch.startIndex]
                if c.isASCII {
                    // 字母统一小写（引擎按小写编码）；大写走 shift 混输
                    keyName = String(c.isLetter && !shift ? Character(c.lowercased()) : c)
                }
            }
        }
        guard let name = keyName else { return false }
        // 修饰键本身不产生编码字符
        if ["shift", "ctrl", "alt"].contains(name) { return false }

        guard let resp = EngineClient.shared.call("key", key: name, shift: shift, ctrl: ctrl, alt: alt),
              let outcome = resp["outcome"] as? [String: Any],
              let consumed = outcome["consumed"] as? Bool else {
            return false // 引擎不可用：透传
        }
        if !consumed { return false }

        let commit = outcome["commit"] as? String
        let state = outcome["state"] as? [String: Any]
        applyOutcome(commit: commit, state: state, client: sender)
        return true
    }

    // MARK: - 组段与上屏

    func applyOutcome(commit: String?, state: [String: Any]?, client sender: Any?) {
        guard let client = sender as? (any IMKTextInput) else { return }
        let raw = state?["raw"] as? String ?? ""
        let preedit = state?["preedit"] as? String ?? ""

        if let commit = commit, !commit.isEmpty {
            if !raw.isEmpty && !preedit.isEmpty {
                client.insertText(commit, replacementRange: .notFound)
            } else {
                client.insertText(commit, replacementRange: .notFound)
            }
            _ = EngineClient.shared.call("reset")
            panel?.hide()
            return
        }
        if raw.isEmpty {
            client.setMarkedText("", selectionRange: NSRange(location: 0, length: 0),
                                 replacementRange: NSRange(location: .max, length: 0))
            panel?.hide()
            return
        }
        // 内联编码（inline_preedit）：把编码放进组段文本
        client.setMarkedText(preedit.isEmpty ? raw : preedit,
                             selectionRange: NSRange(location: 0, length: 0),
                             replacementRange: NSRange(location: .max, length: 0))
        // 候选窗跟随插入点
        let attrs = client.attributes(forCharacterIndex: 0, actualRange: nil)
        if let frame = (attrs as? [String: Any])?["NSCharacterFrame"] as? NSValue {
            showPanel(at: frame.cgRectValue, state: state)
        }
    }

    // MARK: - 候选窗

    func showPanel(at caret: CGRect, state: [String: Any]?) {
        if panel == nil { panel = CandidatePanel() }
        let cands = (state?["candidates"] as? [[String: Any]])?
            .compactMap { c in
                (c["text"] as? String).map { ($0, c["comment"] as? String ?? "") }
            } ?? []
        panel?.update(cands: cands, raw: state?["raw"] as? String ?? "")
        panel?.showBelow(caret)
    }

    override func deactivateServer(_ sender: Any!) {
        _ = EngineClient.shared.call("reset")
        panel?.hide()
        super.deactivateServer(sender)
    }
}
