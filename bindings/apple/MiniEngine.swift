import Foundation
import MiniEngineC

/// Synchronous engine interface. Run requests on an application-owned background
/// queue; requests serialize here, while stop() can run from another thread.
/// close() cancels an admitted request and waits for its completion before freeing
/// the handle. Sendability relies on this invariant: every mutable handle/lifetime
/// field is guarded by `lifetime`, native requests are serialized by `requests`,
/// and every C use pins the handle until it returns. stop bypasses only the
/// request lock; the C ABI guarantees its atomic cancellation operation is safe.
public final class MiniEngine: @unchecked Sendable {
    public enum EngineError: Error {
        case allocationFailed
        case closed
        case invalidResponse
        case engine(String)
    }

    public struct SearchResult: Decodable, Sendable {
        public let bestmove: String
        public let ponder: String?
        public let score: Int
        public let depth: Int
        public let nodes: UInt64
        public let elapsed_ms: UInt64
        public let hashfull: Int
        public let pv: [String]
    }

    public struct Response: Decodable, Sendable {
        public let ok: Bool
        public let error: String?
        public let digest: String?
        public let score: Int?
        public let result: SearchResult?
    }

    private let requests = NSLock()
    private let lifetime = NSCondition()
    private var handle: OpaquePointer?
    private var active = 0

    public init() throws {
        guard let handle = mini_engine_new() else { throw EngineError.allocationFailed }
        self.handle = handle
    }

    deinit { close() }

    /// Idempotent. May block until the current native request returns.
    public func close() {
        lifetime.lock()
        guard let handle = handle else {
            lifetime.unlock()
            return
        }
        self.handle = nil // Refuse admission before cancelling or waiting.
        mini_engine_stop(handle)
        while active != 0 { lifetime.wait() }
        mini_engine_free(handle)
        lifetime.unlock()
    }

    private func withHandle<T>(_ body: (OpaquePointer) throws -> T) throws -> T {
        lifetime.lock()
        guard let handle = handle else {
            lifetime.unlock()
            throw EngineError.closed
        }
        active += 1
        lifetime.unlock()
        defer {
            lifetime.lock()
            active -= 1
            lifetime.broadcast()
            lifetime.unlock()
        }
        return try body(handle)
    }

    /// Uses separate handle admission, never the request lock held by search.
    public func stop() {
        try? withHandle { mini_engine_stop($0) }
    }

    private func decode(_ pointer: UnsafeMutablePointer<CChar>?) throws -> Response {
        guard let pointer = pointer else { throw EngineError.invalidResponse }
        defer { mini_string_free(pointer) }
        let response = try JSONDecoder().decode(Response.self, from: Data(String(cString: pointer).utf8))
        guard response.ok else { throw EngineError.engine(response.error ?? "unknown engine error") }
        return response
    }

    public func request(_ command: [String: Any]) throws -> Response {
        let data = try JSONSerialization.data(withJSONObject: command)
        guard let json = String(data: data, encoding: .utf8) else { throw EngineError.invalidResponse }
        requests.lock()
        defer { requests.unlock() }
        return try withHandle { handle in
            try json.withCString { try decode(mini_engine_request(handle, $0)) }
        }
    }

    public func loadModel(_ data: Data) throws -> Response {
        requests.lock()
        defer { requests.unlock() }
        return try withHandle { handle in
            try data.withUnsafeBytes { bytes in
                try decode(mini_engine_load_model(handle, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count))
            }
        }
    }

    public func loadEmbedded() throws -> Response { try request(["command": "load_embedded"]) }
    public func newGame() throws { _ = try request(["command": "new_game"]) }
    public func setPosition(sfen: String = "startpos", moves: [String] = []) throws {
        _ = try request(["command": "set_position", "sfen": sfen, "moves": moves])
    }
    public func configure(hashMB: Int? = nil, threads: Int? = nil) throws {
        var command: [String: Any] = ["command": "configure"]
        if let hashMB = hashMB { command["hash_mb"] = hashMB }
        if let threads = threads { command["threads"] = threads }
        _ = try request(command)
    }
    public func evaluate() throws -> Int {
        guard let score = try request(["command": "evaluate"]).score else { throw EngineError.invalidResponse }
        return score
    }
    public func search(depth: Int, nodes: UInt64? = nil, movetimeMS: UInt64? = nil) throws -> SearchResult {
        var command: [String: Any] = ["command": "search", "depth": depth]
        if let nodes = nodes { command["nodes"] = nodes }
        if let movetimeMS = movetimeMS { command["movetime_ms"] = movetimeMS }
        guard let result = try request(command).result else { throw EngineError.invalidResponse }
        return result
    }
}
