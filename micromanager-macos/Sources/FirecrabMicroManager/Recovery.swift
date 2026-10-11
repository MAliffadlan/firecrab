import Darwin
import Foundation
@preconcurrency import Virtualization

final class VMRunLock {
    private var descriptor: Int32

    init(home: URL) throws {
        let url = home.appendingPathComponent("runtime/vm.lock")
        descriptor = open(url.path, O_RDWR | O_CREAT | O_NOFOLLOW, S_IRUSR | S_IWUSR)
        guard descriptor >= 0 else {
            throw MicroManagerError.invalidArtifact("cannot open management VM lock: \(url.path)")
        }
        guard flock(descriptor, LOCK_EX | LOCK_NB) == 0 else {
            close()
            throw MicroManagerError.invalidArtifact("management VM or OS repair already owns \(url.path)")
        }
    }

    func close() {
        if descriptor >= 0 {
            Darwin.close(descriptor)
            descriptor = -1
        }
    }

    deinit { close() }
}

struct OSRepairResult: Codable, Equatable {
    let check_exit: Int
    let repair_exit: Int?
    let verify_exit: Int
    let changed: Bool

    private enum CodingKeys: CodingKey {
        case check_exit, repair_exit, verify_exit, changed
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(check_exit, forKey: .check_exit)
        if let repair_exit {
            try container.encode(repair_exit, forKey: .repair_exit)
        } else {
            try container.encodeNil(forKey: .repair_exit)
        }
        try container.encode(verify_exit, forKey: .verify_exit)
        try container.encode(changed, forKey: .changed)
    }

    var verified: Bool {
        verify_exit == 0 && (changed
            ? (1...7).contains(check_exit) && repair_exit.map { (0...3).contains($0) } == true
            : check_exit == 0 && repair_exit == nil)
    }
}

func recoveryOptions(_ options: VMOptions, disk: URL) throws -> VMOptions {
    let home = options.managedHomeURL.standardizedFileURL.resolvingSymlinksInPath()
    let directory = home.appendingPathComponent("runtime/repairs", isDirectory: true).path + "/"
    let target = disk.standardizedFileURL
    guard target == target.resolvingSymlinksInPath(), target.path.hasPrefix(directory),
        target.lastPathComponent == "debian-system.repaired.raw"
    else {
        throw MicroManagerError.argument("repair-os accepts only a private OS copy under runtime/repairs")
    }
    let metadata = try FileManager.default.attributesOfItem(atPath: target.path)
    guard metadata[.type] as? FileAttributeType == .typeRegular,
        (metadata[.referenceCount] as? NSNumber)?.intValue == 1
    else {
        throw MicroManagerError.invalidArtifact("repair-os requires an unaliased regular OS copy")
    }
    try validateSeparateDisks(target, options.osDiskURL)
    try validateSeparateDisks(target, options.dataDiskURL)
    return VMOptions(
        managedHomeURL: options.managedHomeURL,
        kernelURL: options.kernelURL,
        initialRamdiskURL: options.initialRamdiskURL,
        osDiskURL: target,
        dataDiskURL: options.dataDiskURL,
        cpuCount: 2,
        memorySizeMiB: 1024,
        commandLine: options.commandLine + " break=premount TERM=dumb"
    )
}

@MainActor
final class OSRepairConsole {
    let marker = "FIRECRAB_FSCK_" + UUID().uuidString.replacingOccurrences(of: "-", with: "")
    private let input: FileHandle
    private var buffer = ""
    private var sent = false
    private(set) var result: OSRepairResult?

    init(input: FileHandle) { self.input = input }

    var command: String {
        // premount stops before the OS root is mounted. Reject any mounted OS
        // device anyway, and require a second read-only pass after corrections.
        "stty -echo; repair=-1; "
            + "if [ ! -b /dev/vda1 ] || grep -qE '^/dev/(vda|root)|^[^ ]+ /root ' /proc/mounts; then check=8; verify=8; "
            + "else /sbin/fsck.ext4 -f -n /dev/vda1; check=$?; "
            + "if [ \"$check\" -gt 0 ] && [ \"$check\" -le 7 ]; then "
            + "/sbin/fsck.ext4 -f -y /dev/vda1; repair=$?; "
            + "if [ \"$repair\" -le 3 ]; then /sbin/fsck.ext4 -f -n /dev/vda1; verify=$?; else verify=$repair; fi; "
            + "else verify=$check; fi; fi; sync; "
            + "printf '\\n\(marker):%s:%s:%s\\n' \"$check\" \"$repair\" \"$verify\"; poweroff -f\n"
    }

    func consume(_ data: Data) {
        FileHandle.standardError.write(data)
        // Swift treats CRLF as one Character, so normalize serial-console line endings.
        buffer += String(decoding: data, as: UTF8.self).replacingOccurrences(of: "\r", with: "\n")
        if !sent && buffer.contains("(initramfs)") {
            sent = true
            input.write(Data(command.utf8))
        }
        while let newline = buffer.firstIndex(of: "\n") {
            let line = String(buffer[..<newline]).trimmingCharacters(in: .whitespacesAndNewlines)
            buffer.removeSubrange(...newline)
            guard line.hasPrefix(marker + ":") else { continue }
            let fields = line.split(separator: ":")
            guard fields.count == 4, let check = Int(fields[1]),
                let repair = Int(fields[2]), let verify = Int(fields[3]) else { continue }
            result = OSRepairResult(check_exit: check, repair_exit: repair < 0 ? nil : repair,
                verify_exit: verify, changed: repair >= 0)
        }
        if buffer.count > 8192 { buffer = String(buffer.suffix(8192)) }
    }
}

@MainActor
func repairOS(options: VMOptions, disk: URL) async throws -> OSRepairResult {
    let options = try recoveryOptions(options, disk: disk)
    let input = Pipe()
    let output = Pipe()
    let console = OSRepairConsole(input: input.fileHandleForWriting)
    let configuration = try ManagementVMConfiguration.make(options: options,
        consoleInput: input.fileHandleForReading, consoleOutput: output.fileHandleForWriting,
        repairingOS: true)
    output.fileHandleForReading.readabilityHandler = { handle in
        let data = handle.availableData
        if !data.isEmpty {
            Task { @MainActor in console.consume(data) }
        }
    }
    defer {
        output.fileHandleForReading.readabilityHandler = nil
        try? input.fileHandleForWriting.close()
        try? output.fileHandleForWriting.close()
    }
    try await runManagementVM(configuration: configuration)
    // The delegate can report shutdown before the final serial chunk is queued.
    for _ in 0..<20 {
        if let result = console.result { return result }
        try await Task.sleep(for: .milliseconds(50))
    }
    throw MicroManagerError.invalidArtifact("OS repair VM stopped without a filesystem result")
}
