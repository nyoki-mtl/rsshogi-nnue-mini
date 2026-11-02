import Foundation
import Dispatch

// All mutable test outcomes are protected by the lock, including reads.
private final class SearchOutcome: @unchecked Sendable {
    private let lock = NSLock()
    private var value: String?
    func fail(_ message: String) { lock.lock(); value = message; lock.unlock() }
    func failure() -> String? { lock.lock(); defer { lock.unlock() }; return value }
}

@main
struct CheckApple {
    static func require(_ condition: Bool, _ message: String) throws {
        if !condition { throw NSError(domain: "MiniEngineSmoke", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
    }

    static func expectError(_ action: () throws -> Void) throws {
        var failed = false
        do { try action() } catch { failed = true }
        try require(failed, "expected an engine error")
    }

    static func main() throws {
        let engine = try MiniEngine()
        defer { engine.close() }
        try expectError { _ = try engine.evaluate() }
        try expectError { _ = try engine.loadModel(Data("invalid model".utf8)) }
        try expectError { try engine.configure(hashMB: 0) }
        try expectError { try engine.setPosition(moves: ["7g7e"]) }
        try engine.configure(hashMB: 4, threads: 1)
        try engine.setPosition()
        try engine.newGame()
        if let path = ProcessInfo.processInfo.environment["RSSHOGI_EMBEDDED_RSNN_PATH"] {
            _ = try engine.loadModel(Data(contentsOf: URL(fileURLWithPath: path)))
            let score = try engine.evaluate()
            engine.stop()
            let cancelled = try engine.search(depth: 64, movetimeMS: 2000)
            try require(cancelled.depth == 0, "pending stop was lost")
            let result = try engine.search(depth: 1)
            try require(result.depth == 1 && result.pv.first == result.bestmove, "invalid depth-1 result")
            try require(try engine.evaluate() == score, "search changed evaluation position")
            let stopGroup = DispatchGroup()
            let outcome = SearchOutcome()
            stopGroup.enter()
            DispatchQueue.global().async {
                defer { stopGroup.leave() }
                do { _ = try engine.search(depth: 64, movetimeMS: 2000) }
                catch { outcome.fail(String(describing: error)) }
            }
            Thread.sleep(forTimeInterval: 0.05)
            stopGroup.enter()
            DispatchQueue.global().async { engine.stop(); stopGroup.leave() }
            try require(stopGroup.wait(timeout: .now() + 1) == .success, "concurrent stop timed out")
            try require(outcome.failure() == nil, outcome.failure() ?? "search failed")
            try require(try engine.search(depth: 1).depth == 1, "stop was not consumed")
            let closeGroup = DispatchGroup()
            closeGroup.enter()
            DispatchQueue.global().async {
                defer { closeGroup.leave() }
                do { _ = try engine.search(depth: 64, movetimeMS: 2000) }
                catch MiniEngine.EngineError.closed { /* Closing before admission is allowed. */ }
                catch { outcome.fail(String(describing: error)) }
            }
            Thread.sleep(forTimeInterval: 0.05)
            closeGroup.enter()
            DispatchQueue.global().async { engine.close(); closeGroup.leave() }
            try require(closeGroup.wait(timeout: .now() + 1) == .success, "concurrent close timed out")
            try require(outcome.failure() == nil, outcome.failure() ?? "search failed")
            print("Swift real-model load, evaluation, pending stop, and search: OK")
        }
        engine.stop()
        engine.close()
        engine.close()
        try expectError { _ = try engine.evaluate() }
        print("Swift wrapper lifecycle and error handling: OK")
    }
}
