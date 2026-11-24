import XCTest
@testable import RemoteTransport

final class ClientTests: XCTestCase {
    func testOriginBindsAuthorityAndRejectsExtras() throws {
        XCTAssertEqual(try R3HTTPSOrigin("https://Store.Example/").identity, "https://store.example:443")
        XCTAssertEqual(try R3HTTPSOrigin("https://[::1]:9443").identity, "https://[::1]:9443")
        for bad in ["http://store.example", "https://store.example/path", "https://a@store.example",
                    "https://store.example?", "https://store.example#", "https://store.example:",
                    "https://127.1", "https://[::1%25scope]", "https://store.example:0"] {
            XCTAssertThrowsError(try R3HTTPSOrigin(bad))
        }
    }

    func testCancelledCallMayNotResolveCredentialOrWrite() throws {
        let permission = SendPermission()
        permission.cancel()
        XCTAssertThrowsError(try permission.use { XCTFail("action ran after cancel") })
        XCTAssertThrowsError(try permission.use { XCTFail("second action ran after cancel") })
    }
}
