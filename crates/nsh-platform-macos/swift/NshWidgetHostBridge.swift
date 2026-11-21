import Foundation
import WidgetKit

/// Reloads timelines for one widget kind via `WidgetCenter.shared`.
@_cdecl("nsh_macos_widget_reload_kind")
public func nsh_macos_widget_reload_kind(_ kind: UnsafePointer<CChar>?) {
    guard let kind else { return }
    let id = String(cString: kind)
    guard !id.isEmpty else { return }
    WidgetCenter.shared.reloadTimelines(ofKind: id)
}

/// Reloads all timelines for the current app's WidgetKit extensions.
@_cdecl("nsh_macos_widget_reload_all")
public func nsh_macos_widget_reload_all() {
    WidgetCenter.shared.reloadAllTimelines()
}

/// Writes the shared App Group container directory path as UTF-8 (NUL-terminated)
/// into `outBuf` (capacity `outLen`). Returns the path length in bytes (excluding
/// NUL), or `0` if the container is unavailable or does not fit.
@_cdecl("nsh_macos_widget_group_container")
public func nsh_macos_widget_group_container(
    _ groupId: UnsafePointer<CChar>?,
    _ outBuf: UnsafeMutablePointer<CChar>?,
    _ outLen: Int
) -> Int {
    guard let groupId else { return 0 }
    let id = String(cString: groupId)
    guard !id.isEmpty,
          let url = FileManager.default.containerURL(
              forSecurityApplicationGroupIdentifier: id
          )
    else {
        return 0
    }
    let path = url.path
    let utf8 = Array(path.utf8)
    guard let outBuf, outLen > utf8.count else {
        return utf8.count
    }
    for i in 0..<utf8.count {
        outBuf[i] = CChar(bitPattern: utf8[i])
    }
    outBuf[utf8.count] = 0
    return utf8.count
}
