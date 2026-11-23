import Foundation
import XCTest
@testable import ZipPreview

final class LocalDraftTests: XCTestCase {
    private func withStore(_ body: (URL, LocalDraft) throws -> Void) throws {
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        try body(folder, LocalDraft(folder: folder))
    }

    func testRelaunchAndStaleEditorKeepExactNewerBytes() throws {
        try withStore { folder, store in
            XCTAssertNil(try store.open())
            let first = try store.save("e\u{301}\r\n", replacing: nil)
            let secondSession = LocalDraft(folder: folder)
            XCTAssertEqual(try secondSession.open()?.bytes, Data("e\u{301}\r\n".utf8))
            let current = try secondSession.save("é\n", replacing: first)
            XCTAssertThrowsError(try store.save("stale", replacing: first)) {
                XCTAssertEqual($0 as? LocalDraft.Failure, .changed)
            }
            XCTAssertEqual(try store.open(), current)
            XCTAssertEqual(try store.open()?.bytes, Data("é\n".utf8))
        }
    }

    func testBoundsAndCorruptionKeepPreviousFile() throws {
        try withStore { folder, store in
            let original = try store.save(String(repeating: "a", count: LocalDraft.byteLimit), replacing: nil)
            XCTAssertThrowsError(try store.save(original.text + "字", replacing: original))
            XCTAssertEqual(try store.open(), original)
            let saved = folder.appendingPathComponent("text.utf8")
            try Data([0xff]).write(to: saved)
            XCTAssertThrowsError(try store.open())
            XCTAssertThrowsError(try store.save("replacement", replacing: nil))
            XCTAssertEqual(try Data(contentsOf: saved), Data([0xff]))
        }
    }
}
