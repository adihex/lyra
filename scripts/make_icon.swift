// Compatibility entry point only. The original Cosmos scene now lives in the
// app's CosmosScene/CosmosPaintCG/PetPaint sources; it is no longer the app icon.
// Use `make icons` or `python3 scripts/make_icon.py` on either platform.
import Foundation

let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
if CommandLine.arguments.count > 1 {
    fputs("The PNG output argument is retired; use make icons (see README.md).\n", stderr)
    exit(1)
}
let process = Process()
process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
process.arguments = ["python3", root.appendingPathComponent("scripts/make_icon.py").path]
try process.run()
process.waitUntilExit()
exit(process.terminationStatus)
