import XCTest
@testable import RemoteTransport

final class ConnectionGateTests: XCTestCase {
    func testTLSGateAcceptsOnlyExactH2Token() {
        XCTAssertTrue(NegotiatedHTTP2.matchesHTTP2("h2"))
        for other in [nil, "", "h2c", "H2", "http/1.1", "h2 " ] as [String?] {
            XCTAssertFalse(NegotiatedHTTP2.matchesHTTP2(other))
        }
    }

    func testAddressCannotCarryURLSyntaxOrInvalidPort() throws {
        let address = try TransportAddress(host: "store.example", port: 443)
        XCTAssertEqual(address.host, "store.example")
        XCTAssertThrowsError(try TransportAddress(host: "https://store.example", port: 443))
        XCTAssertThrowsError(try TransportAddress(host: "abc@store.example", port: 443))
        XCTAssertThrowsError(try TransportAddress(host: "store.example", port: 0))
        XCTAssertThrowsError(try TransportAddress(host: "store.example", port: 65536))
    }
}
