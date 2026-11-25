import Foundation
import CoreText
import CoreGraphics

/// Enumerates all system and user-installed fonts visible to CoreText on iPadOS
/// and registers them with WordCraft's `FontDb` (by file path when directly readable,
/// or by assembling SFNT table data from `CGFont` for sandbox-brokered user font apps).
enum FontScanner {
    static func registerInstalledFonts(host: OpaquePointer) {
        DispatchQueue.global(qos: .utility).async {
            let collection = CTFontCollectionCreateFromAvailableFonts(nil)
            guard let descriptors = CTFontCollectionCreateMatchingFontDescriptors(collection) as? [CTFontDescriptor] else {
                return
            }

            var seenPaths = Set<String>()
            var seenNames = Set<String>()
            var pathsToRegister: [String] = []
            var bytesToRegister: [Data] = []

            for desc in descriptors {
                if let url = CTFontDescriptorCopyAttribute(desc, kCTFontURLAttribute) as? URL,
                   url.isFileURL {
                    let path = url.path
                    if seenPaths.contains(path) {
                        continue
                    }
                    seenPaths.insert(path)

                    if FileManager.default.isReadableFile(atPath: path) {
                        pathsToRegister.append(path)
                        continue
                    }
                }

                // Fallback for sandbox-protected user fonts (Settings / iFont / Creative Cloud)
                // whose file URL isn't directly readable via POSIX open(): extract SFNT tables via CGFont.
                let psName = (CTFontDescriptorCopyAttribute(desc, kCTFontNameAttribute) as? String) ?? ""
                if !psName.isEmpty {
                    if seenNames.contains(psName) {
                        continue
                    }
                    seenNames.insert(psName)
                }

                let ctFont = CTFontCreateWithFontDescriptor(desc, 0.0, nil)
                let cgFont = CTFontCopyGraphicsFont(ctFont, nil)
                if let sfnt = serializeSFNT(cgFont) {
                    bytesToRegister.append(sfnt)
                }
            }

            // Register on main thread cleanly without risking concurrent access to `WordCraftHost`.
            let chunkSize = 32
            for start in stride(from: 0, to: pathsToRegister.count, by: chunkSize) {
                let batch = Array(pathsToRegister[start..<min(start + chunkSize, pathsToRegister.count)])
                DispatchQueue.main.async {
                    for path in batch {
                        path.withCString { cPath in
                            _ = wordcraft_register_font_path(host, cPath)
                        }
                    }
                }
            }

            for fontData in bytesToRegister {
                DispatchQueue.main.async {
                    fontData.withUnsafeBytes { rawBuf in
                        if let ptr = rawBuf.baseAddress?.assumingMemoryBound(to: UInt8.self) {
                            _ = wordcraft_register_font_bytes(host, ptr, fontData.count)
                        }
                    }
                }
            }
        }
    }

    /// Reconstructs an OpenType / TrueType SFNT container from a `CGFont`'s tables.
    private static func serializeSFNT(_ cgFont: CGFont) -> Data? {
        guard let cfTags = cgFont.tableTags else { return nil }
        let count = CFArrayGetCount(cfTags)
        if count <= 0 { return nil }

        struct TableRecord {
            let tag: UInt32
            var data: Data
        }

        var tables: [TableRecord] = []
        var hasCFF = false
        for i in 0..<count {
            let rawPtr = CFArrayGetValueAtIndex(cfTags, i)
            let tag = UInt32(truncatingIfNeeded: UInt(bitPattern: rawPtr))
            guard let cfData = cgFont.table(for: tag) else { continue }
            let data = cfData as Data
            if tag == 0x43464620 || tag == 0x43464632 { // 'CFF ' or 'CFF2'
                hasCFF = true
            }
            tables.append(TableRecord(tag: tag, data: data))
        }

        guard !tables.isEmpty else { return nil }
        tables.sort { $0.tag < $1.tag }

        let numTables = UInt16(tables.count)
        let sfntVersion: UInt32 = hasCFF ? 0x4F54544F : 0x00010000 // 'OTTO' vs TrueType 1.0

        var maxPowerOf2: UInt16 = 1
        var entrySelector: UInt16 = 0
        while maxPowerOf2 * 2 <= numTables {
            maxPowerOf2 *= 2
            entrySelector += 1
        }
        let searchRange = maxPowerOf2 * 16
        let rangeShift = numTables * 16 - searchRange

        var out = Data()
        appendBE(&out, sfntVersion)
        appendBE(&out, numTables)
        appendBE(&out, searchRange)
        appendBE(&out, entrySelector)
        appendBE(&out, rangeShift)

        var offset = UInt32(12 + Int(numTables) * 16)
        var headOffset: Int?

        for table in tables {
            var tableBytes = table.data
            if table.tag == 0x68656164 && tableBytes.count >= 12 { // 'head'
                headOffset = Int(offset)
                tableBytes.replaceSubrange(8..<12, with: [0, 0, 0, 0])
            }
            let length = UInt32(tableBytes.count)
            let checksum = calcTableChecksum(tableBytes)
            appendBE(&out, table.tag)
            appendBE(&out, checksum)
            appendBE(&out, offset)
            appendBE(&out, length)

            let paddedLen = (tableBytes.count + 3) & ~3
            offset += UInt32(paddedLen)
        }

        for table in tables {
            var tableBytes = table.data
            if table.tag == 0x68656164 && tableBytes.count >= 12 {
                tableBytes.replaceSubrange(8..<12, with: [0, 0, 0, 0])
            }
            out.append(tableBytes)
            let pad = (4 - (tableBytes.count % 4)) % 4
            if pad > 0 {
                out.append(contentsOf: [UInt8](repeating: 0, count: pad))
            }
        }

        if let hOff = headOffset, hOff + 12 <= out.count {
            let wholeChecksum = calcTableChecksum(out)
            let adjustment = UInt32(0xB1B0AFBA) &- wholeChecksum
            let adjBytes = withUnsafeBytes(of: adjustment.bigEndian) { Array($0) }
            out.replaceSubrange((hOff + 8)..<(hOff + 12), with: adjBytes)
        }

        return out
    }

    private static func appendBE<T: FixedWidthInteger>(_ data: inout Data, _ val: T) {
        var be = val.bigEndian
        withUnsafeBytes(of: &be) { data.append(contentsOf: $0) }
    }

    private static func calcTableChecksum(_ data: Data) -> UInt32 {
        var sum: UInt32 = 0
        var idx = 0
        let count = data.count
        while idx < count {
            var word: UInt32 = 0
            for b in 0..<4 {
                word <<= 8
                if idx + b < count {
                    word |= UInt32(data[idx + b])
                }
            }
            sum = sum &+ word
            idx += 4
        }
        return sum
    }
}
