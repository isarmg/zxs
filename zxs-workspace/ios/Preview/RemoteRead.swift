import CryptoKit
import Foundation

enum RemoteRead {
    enum Failure: Error { case invalidPage, unsupportedNode, unchanged }

    struct Catalog: Decodable {
        let items: [Container]
        let nextAfter: String?

        struct Container: Decodable {
            let containerId: String
            let sequence: UInt64
            let name: String

            var wire: String {
                #"{"container_id":\#(RemoteRead.quoted(containerId)),"sequence":\#(sequence),"name":\#(RemoteRead.quoted(name))}"#
            }
        }

        static func select(_ bytes: Data, after: String?) throws -> Self {
            let value: Self = try RemoteRead.decode(bytes)
            guard value.items.count <= 256, RemoteRead.sorted(value.items.map(\.containerId), after: after),
                  value.items.allSatisfy({ RemoteRead.validID($0.containerId) && $0.sequence > 0 && RemoteRead.display($0.name) }),
                  value.nextAfter == (value.items.count == 256 ? value.items.last?.containerId : nil) else {
                throw Failure.invalidPage
            }
            let objects = value.items.map(\.wire).joined(separator: ",")
            try RemoteRead.exact(bytes, #"{"items":[\#(objects)],"next_after":\#(RemoteRead.maybe(value.nextAfter))}"#)
            return value
        }
    }

    struct Detail: Decodable {
        let containerId: String
        let sequence: UInt64
        let name: String
        let tags: [String]
        let heads: [Head]
        let nextAfter: String?

        struct Head: Decodable {
            let branch: String
            let head: String
            var wire: String { #"{"branch":\#(RemoteRead.quoted(branch)),"head":\#(RemoteRead.quoted(head))}"# }
        }

        static func select(_ bytes: Data, expected: Catalog.Container, after: String?) throws -> Self {
            let value: Self = try RemoteRead.decode(bytes)
            guard value.containerId == expected.containerId, value.sequence == expected.sequence,
                  value.name.utf8.elementsEqual(expected.name.utf8), value.heads.count <= 256,
                  RemoteRead.sorted(value.heads.map(\.branch), after: after),
                  value.heads.allSatisfy({ RemoteRead.validBranch($0.branch) && RemoteRead.validID($0.head) }),
                  value.nextAfter == (value.heads.count == 256 ? value.heads.last?.branch : nil)
            else { throw Failure.invalidPage }
            let tags = value.tags.map(RemoteRead.quoted).joined(separator: ",")
            let heads = value.heads.map(\.wire).joined(separator: ",")
            try RemoteRead.exact(bytes, #"{"container_id":\#(RemoteRead.quoted(value.containerId)),"sequence":\#(value.sequence),"name":\#(RemoteRead.quoted(value.name)),"tags":[\#(tags)],"heads":[\#(heads)],"next_after":\#(RemoteRead.maybe(value.nextAfter))}"#)
            return value
        }
    }

    struct History: Decodable {
        let head: String
        let nodes: [Node]
        let next: String?

        struct Node: Decodable, Sendable {
            let nodeId: String
            let operationId: String
            let parentNode: String?
            let originalBranch: String
            let sequence: UInt64
            let length: UInt64
            let sha256: String

            var wire: String {
                #"{"node_id":\#(RemoteRead.quoted(nodeId)),"operation_id":\#(RemoteRead.quoted(operationId)),"parent_node":\#(RemoteRead.maybe(parentNode)),"original_branch":\#(RemoteRead.quoted(originalBranch)),"sequence":\#(sequence),"length":\#(length),"sha256":\#(RemoteRead.quoted(sha256))}"#
            }
        }

        static func select(_ bytes: Data, head: String, cursor: String?, upper: UInt64) throws -> Self {
            let value: Self = try RemoteRead.decode(bytes)
            guard value.head == head, (1...256).contains(value.nodes.count),
                  value.nodes.first?.nodeId == (cursor ?? head),
                  Set(value.nodes.map(\.nodeId)).count == value.nodes.count,
                  value.nodes.allSatisfy({ RemoteRead.validID($0.nodeId) && RemoteRead.validID($0.operationId)
                      && ($0.parentNode.map(RemoteRead.validID) ?? true) && RemoteRead.validBranch($0.originalBranch)
                      && $0.sequence > 0 && $0.sequence <= upper && RemoteRead.isHex($0.sha256, width: 64) }),
                  value.next == value.nodes.last?.parentNode,
                  value.next == nil || value.nodes.count == 256
            else { throw Failure.invalidPage }
            for (newer, older) in zip(value.nodes, value.nodes.dropFirst()) {
                guard newer.sequence > older.sequence, newer.parentNode == older.nodeId
                else { throw Failure.invalidPage }
            }
            if let next = value.next, value.nodes.contains(where: { $0.nodeId == next }) { throw Failure.invalidPage }
            let nodes = value.nodes.map(\.wire).joined(separator: ",")
            try RemoteRead.exact(bytes, #"{"head":\#(RemoteRead.quoted(value.head)),"nodes":[\#(nodes)],"next":\#(RemoteRead.maybe(value.next))}"#)
            return value
        }
    }

    struct TextSelection {
        let container: Catalog.Container
        let branch: Detail.Head
        let node: History.Node
        let original: Data

        init(container: Catalog.Container, branch: Detail.Head, node: History.Node, original: Data) throws {
            guard node.length == UInt64(original.count), original.count <= LocalDraft.byteLimit
            else { throw Failure.unsupportedNode }
            let digest = SHA256.hash(data: original).map { String(format: "%02x", $0) }.joined()
            guard digest == node.sha256,
                  String(data: original, encoding: .utf8) != nil,
                  node.nodeId == branch.head || (branch.branch == "main" && node.originalBranch == "main")
            else { throw Failure.unsupportedNode }
            self.container = container
            self.branch = branch
            self.node = node
            self.original = original
        }

        func displayedExactly(_ visible: String) -> Bool { original.elementsEqual(visible.utf8) }

        func freeze(text: String, destination: String, operationID: String, newNode: String, newBranch: String) throws -> RemoteSave {
            let body = Data(text.utf8)
            guard body != original else { throw Failure.unchanged }
            let target: RemoteSave.Target = node.nodeId == branch.head
                ? .append(branch: branch.branch, head: node.nodeId)
                : .forkMain(branch: newBranch, from: node.nodeId)
            return try RemoteSave(destination: destination, container: container.containerId, operation: operationID,
                node: newNode, base: container.sequence, target: target, draft: LocalDraft.Revision(bytes: body))
        }
    }

    private static func decode<T: Decodable>(_ bytes: Data) throws -> T {
        guard bytes.count <= 1024 * 1024, String(data: bytes, encoding: .utf8) != nil else { throw Failure.invalidPage }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(T.self, from: bytes)
    }

    private static func exact(_ bytes: Data, _ wire: String) throws {
        guard bytes.elementsEqual(wire.utf8) else { throw Failure.invalidPage }
    }

    private static func maybe(_ field: String?) -> String { field.map(quoted) ?? "null" }

    // Match the server's exact JSON output, not Foundation's possibly different
    // encoder. Reject duplicate/additional keys by whole-response comparison.
    private static func quoted(_ value: String) -> String {
        var result = "\""
        for scalar in value.unicodeScalars {
            switch scalar.value {
            case 34: result += "\\\""
            case 92: result += "\\\\"
            case 0...31: result += String(format: "\\u%04x", scalar.value)
            default: result.unicodeScalars.append(scalar)
            }
        }
        return result + "\""
    }

    private static func sorted(_ values: [String], after: String?) -> Bool {
        if let after, let first = values.first, after >= first { return false }
        return zip(values, values.dropFirst()).allSatisfy { $0.0 < $0.1 }
    }

    private static func isHex(_ value: String, width: Int) -> Bool {
        value.utf8.count == width && value.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }

    private static func validID(_ value: String) -> Bool { isHex(value, width: 32) && value.utf8.contains { $0 != 48 } }
    private static func validBranch(_ value: String) -> Bool { value == "main" || validID(value) }
    private static func display(_ value: String) -> Bool { RemoteSave.Target.firstMain(name: value).valid }
}
