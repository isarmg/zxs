import Foundation

/// A platform adapter must derive this identity from the selected TLS authority
/// and selected account. It may not retry or follow a redirect for this call.
protocol RemoteSaveExchange: Sendable {
    var destination: String { get }
    var acceptsFrozenFiles: Bool { get }
    func once(_ request: RemoteSave.Request) async throws -> RemoteSavePacket
}

extension RemoteSaveExchange {
    var acceptsFrozenFiles: Bool { false }
}

struct RemoteSavePacket: Sendable {
    let status: Int
    let bytes: Data
}

/// One instance for the application store. Reentrant awaits may serve other
/// operations but never allow overlapping lookup and submit for one ID.
actor RemoteSaveCoordinator {
    enum Choice: Sendable { case lookup, submitFirst, retrySameOperation }
    enum Result: Equatable, Sendable {
        case absent, requiresExplicitRetry, conflict, unknown, committed(UInt64)
    }
    enum Failure: Error { case busy, selectionChanged }

    private let store: RemoteSaveStore
    private let wholeFiles: WholeFileStage?
    private var running = Set<String>()

    init(store: RemoteSaveStore, wholeFiles: WholeFileStage? = nil) {
        self.store = store; self.wholeFiles = wholeFiles
    }

    func run(consent: RemoteSaveConsent, choice: Choice, over exchange: any RemoteSaveExchange) async throws -> Result {
        let operation = consent.operation
        guard running.insert(operation).inserted else { throw Failure.busy }
        defer { running.remove(operation) }
        try Task.checkCancellation()

        let save = try store.reopenForRecovery(operation: operation)
        guard try consent.permits(save) else { throw Failure.selectionChanged }
        let destination = exchange.destination
        let lookup = try save.lookup(for: destination)
        if let oldReceipt = try store.cachedReceipt(save) {
            guard case let .committed(sequence) = try save.receipt(status: 200, bytes: oldReceipt,
                    isLookup: true, authenticatedDestination: destination) else {
                throw RemoteSave.Failure.invalidStoredRecord
            }
            try store.recordReceipt(oldReceipt, for: save)  // confirm persistence after an uncertain sync
            return .committed(sequence)
        }

        let wasAttempted = try store.attempted(save)
        let observed = try await exchange.once(lookup)
        guard destination.utf8.elementsEqual(exchange.destination.utf8) else { return .unknown }
        switch try save.receipt(status: observed.status, bytes: observed.bytes,
                                isLookup: true, authenticatedDestination: destination) {
        case let .committed(sequence):
            try store.recordReceipt(observed.bytes, for: save)
            return .committed(sequence)
        case .conflict: return .conflict
        case .unknown: return .unknown
        case .absent: break
        }

        switch choice {
        case .lookup: return wasAttempted ? .requiresExplicitRetry : .absent
        case .submitFirst: if wasAttempted { return .requiresExplicitRetry }
        case .retrySameOperation: if !wasAttempted { return .absent }
        }

        try Task.checkCancellation()
        guard destination.utf8.elementsEqual(exchange.destination.utf8) else { return .unknown }
        var held: WholeFileStage.Opened? = nil
        if let diskBody = save.diskBody {
            guard exchange.acceptsFrozenFiles, let wholeFiles else { throw RemoteSave.Failure.invalidIntent }
            held = try await wholeFiles.openForUpload(diskBody)
            try Task.checkCancellation()
            guard destination.utf8.elementsEqual(exchange.destination.utf8) else { return .unknown }
        }
        let upload = try store.admitAttempt(save, after: wasAttempted, authenticatedDestination: destination, file: held)
        try Task.checkCancellation()
        let sent = try await exchange.once(upload)
        guard destination.utf8.elementsEqual(exchange.destination.utf8) else { return .unknown }
        switch try save.receipt(status: sent.status, bytes: sent.bytes,
                                isLookup: false, authenticatedDestination: destination) {
        case let .committed(sequence):
            try store.recordReceipt(sent.bytes, for: save)
            return .committed(sequence)
        case .conflict: return .conflict
        case .absent, .unknown: return .unknown
        }
    }
}
