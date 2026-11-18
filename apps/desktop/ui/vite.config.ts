import { execFile } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

const SUPPORTED_EXTS = new Set([
  "mp4",
  "m4v",
  "mov",
  "webm",
  "mkv",
  "wav",
  "flac",
  "ogg",
  "opus",
  "mp3",
  "m4a",
  "aac",
]);

function mimeForExt(ext: string): string {
  switch (ext) {
    case "mp4":
    case "m4v":
      return "video/mp4";
    case "mov":
      return "video/quicktime";
    case "webm":
      return "video/webm";
    case "mkv":
      return "video/x-matroska";
    case "mp3":
      return "audio/mpeg";
    case "wav":
      return "audio/wav";
    case "flac":
      return "audio/flac";
    case "ogg":
    case "opus":
      return "audio/ogg";
    case "m4a":
      return "audio/mp4";
    case "aac":
      return "audio/aac";
    default:
      return "application/octet-stream";
  }
}

function readExactAt(fd: number, offset: number, length: number): Buffer | null {
  const buf = Buffer.alloc(length);
  try {
    const n = fs.readSync(fd, buf, 0, length, offset);
    return n === length ? buf : null;
  } catch {
    return null;
  }
}

function readMp4BoxHeader(
  fd: number,
  pos: number,
  limit: number,
): { type: string; size: number; hdrLen: number } | null {
  if (pos + 8 > limit) return null;
  const hdr8 = readExactAt(fd, pos, 8);
  if (!hdr8) return null;
  const sz32 = hdr8.readUInt32BE(0);
  const type = hdr8.toString("latin1", 4, 8);
  let size = sz32;
  let hdrLen = 8;
  if (sz32 === 1) {
    if (pos + 16 > limit) return null;
    const ext8 = readExactAt(fd, pos + 8, 8);
    if (!ext8) return null;
    size = Number(ext8.readBigUInt64BE(0));
    hdrLen = 16;
  } else if (sz32 === 0) {
    size = limit - pos;
  }
  if (size < hdrLen || pos + size > limit) return null;
  return { type, size, hdrLen };
}

function probeIsoBmffDurationMs(fd: number, fileLen: number): number | null {
  let pos = 0;
  let sidxSumMs = 0;
  let hasSidx = false;
  let trackedSidxRefId: number | null = null;
  let guard = 0;

  while (pos < fileLen && guard < 4096) {
    guard += 1;
    const box = readMp4BoxHeader(fd, pos, fileLen);
    if (!box) break;

    if (box.type === "moov") {
      const moovEnd = pos + box.size;
      let cpos = pos + box.hdrLen;
      let cguard = 0;
      while (cpos < moovEnd && cguard < 512) {
        cguard += 1;
        const cbox = readMp4BoxHeader(fd, cpos, moovEnd);
        if (!cbox) break;
        if (cbox.type === "mvhd") {
          const payloadLen = cbox.size - cbox.hdrLen;
          const want = Math.min(32, payloadLen);
          if (want >= 20) {
            const mvhd = readExactAt(fd, cpos + cbox.hdrLen, want);
            if (mvhd) {
              const ver = mvhd[0];
              let timescale = 0n;
              let duration = 0n;
              if (ver === 1 && want >= 32) {
                timescale = BigInt(mvhd.readUInt32BE(20));
                const rawDur = mvhd.readBigUInt64BE(24);
                duration = rawDur === 0xffffffffffffffffn ? 0n : rawDur;
              } else if (ver === 0) {
                timescale = BigInt(mvhd.readUInt32BE(12));
                const rawDur = mvhd.readUInt32BE(16);
                duration = rawDur === 0xffffffff ? 0n : BigInt(rawDur);
              }
              if (timescale > 0n && duration > 0n) {
                const ms = Number((duration * 1000n) / timescale);
                if (Number.isFinite(ms) && ms > 0) return ms;
              }
            }
          }
        }
        cpos += cbox.size;
      }
    } else if (box.type === "sidx") {
      const payloadLen = box.size - box.hdrLen;
      if (payloadLen >= 24 && payloadLen <= 2_097_152) {
        const sidx = readExactAt(fd, pos + box.hdrLen, payloadLen);
        if (sidx) {
          const ver = sidx[0];
          const refId = sidx.readUInt32BE(4);
          const timescale = BigInt(sidx.readUInt32BE(8));
          const entriesOffset = ver === 0 ? 20 : 28;
          if (timescale > 0n && sidx.length >= entriesOffset + 4) {
            if (trackedSidxRefId === null) {
              trackedSidxRefId = refId;
            }
            if (refId === trackedSidxRefId) {
              const count = sidx.readUInt16BE(entriesOffset + 2);
              const tableStart = entriesOffset + 4;
              if (sidx.length >= tableStart + count * 12) {
                let sum = 0n;
                for (let i = 0; i < count; i += 1) {
                  sum += BigInt(sidx.readUInt32BE(tableStart + i * 12 + 4));
                }
                if (sum > 0n) {
                  const ms = Number((sum * 1000n) / timescale);
                  if (Number.isFinite(ms) && ms > 0) {
                    sidxSumMs += ms;
                    hasSidx = true;
                  }
                }
              }
            }
          }
        }
      }
    }

    pos += box.size;
  }

  return hasSidx && sidxSumMs > 0 ? sidxSumMs : null;
}

function skipId3v2Tags(fd: number, fileLen: number): number {
  let offset = 0;
  for (let i = 0; i < 8; i += 1) {
    if (offset + 10 > fileLen) break;
    const hdr = readExactAt(fd, offset, 10);
    if (!hdr || hdr.toString("latin1", 0, 3) !== "ID3") break;
    if ((hdr[6]! & 0x80) !== 0 || (hdr[7]! & 0x80) !== 0 || (hdr[8]! & 0x80) !== 0 || (hdr[9]! & 0x80) !== 0) {
      break;
    }
    const tagLen =
      ((hdr[6]! & 0x7f) << 21) |
      ((hdr[7]! & 0x7f) << 14) |
      ((hdr[8]! & 0x7f) << 7) |
      (hdr[9]! & 0x7f);
    const footer = (hdr[5]! & 0x10) !== 0 ? 10 : 0;
    const next = offset + 10 + tagLen + footer;
    if (next <= offset || next >= fileLen) break;
    offset = next;
  }
  return offset;
}

function probeMp3DurationMs(fd: number, fileLen: number): number | null {
  const start = skipId3v2Tags(fd, fileLen);
  const windowLen = Math.min(65536, fileLen - start);
  if (windowLen < 8) return null;
  const buf = readExactAt(fd, start, windowLen);
  if (!buf) return null;
  const sampleRates: Record<number, number[]> = {
    3: [44100, 48000, 32000], // MPEG 1
    2: [22050, 24000, 16000], // MPEG 2
    0: [11025, 12000, 8000], // MPEG 2.5
  };
  const bitratesV1L3 = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
  const bitratesV2L3 = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];

  for (let i = 0; i + 4 <= buf.length; i += 1) {
    if (buf[i] !== 0xff || (buf[i + 1]! & 0xe0) !== 0xe0) continue;
    const verBits = (buf[i + 1]! >> 3) & 0x03;
    const layerBits = (buf[i + 1]! >> 1) & 0x03;
    if (verBits === 1 || layerBits !== 1) continue;
    const brIdx = (buf[i + 2]! >> 4) & 0x0f;
    const srIdx = (buf[i + 2]! >> 2) & 0x03;
    if (brIdx === 0 || brIdx === 15 || srIdx === 3) continue;
    const sr = sampleRates[verBits]?.[srIdx] ?? 0;
    const kbps = verBits === 3 ? (bitratesV1L3[brIdx] ?? 0) : (bitratesV2L3[brIdx] ?? 0);
    if (!sr || !kbps) continue;
    const padding = (buf[i + 2]! >> 1) & 0x01;
    const samplesPerFrame = verBits === 3 ? 1152 : 576;
    const frameLen = Math.floor(((samplesPerFrame / 8) * kbps * 1000) / sr) + padding;
    if (frameLen < 24) continue;
    if (i + frameLen + 2 <= buf.length) {
      if (buf[i + frameLen] !== 0xff || (buf[i + frameLen + 1]! & 0xe0) !== 0xe0) continue;
    }
    const hasCrc = (buf[i + 1]! & 0x01) === 0;
    const chanMode = (buf[i + 3]! >> 6) & 0x03;
    const sideInfo = verBits === 3 ? (chanMode === 3 ? 17 : 32) : chanMode === 3 ? 9 : 17;
    const xingOff = i + 4 + (hasCrc ? 2 : 0) + sideInfo;
    if (xingOff + 12 <= buf.length) {
      const tag = buf.toString("latin1", xingOff, xingOff + 4);
      if ((tag === "Xing" || tag === "Info") && (buf.readUInt32BE(xingOff + 4) & 1) !== 0) {
        const frames = buf.readUInt32BE(xingOff + 8);
        if (frames > 0) return Math.round((frames * samplesPerFrame * 1000) / sr);
      }
    }
    let audioBytes = fileLen - (start + i);
    if (fileLen >= 128) {
      const tail = readExactAt(fd, fileLen - 128, 3);
      if (tail && tail.toString("latin1", 0, 3) === "TAG" && audioBytes > 128) {
        audioBytes -= 128;
      }
    }
    return Math.round((audioBytes * 8) / kbps);
  }
  return null;
}

function probeAacAdtsDurationMs(fd: number, fileLen: number): number | null {
  const start = skipId3v2Tags(fd, fileLen);
  const windowLen = Math.min(65536, fileLen - start);
  if (windowLen < 14) return null;
  const buf = readExactAt(fd, start, windowLen);
  if (!buf) return null;
  const freqs = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
  let pos = 0;
  let sr = 0;
  let frames = 0;
  let bytesSum = 0;
  while (pos + 7 <= buf.length && frames < 128) {
    if (buf[pos] !== 0xff || (buf[pos + 1]! & 0xf6) !== 0xf0) {
      if (frames > 0) break;
      pos += 1;
      continue;
    }
    const sfIdx = (buf[pos + 2]! >> 2) & 0x0f;
    const freq = freqs[sfIdx] ?? 0;
    const frameLen = ((buf[pos + 3]! & 0x03) << 11) | (buf[pos + 4]! << 3) | ((buf[pos + 5]! >> 5) & 0x07);
    if (!freq || frameLen < 7) {
      if (frames > 0) break;
      pos += 1;
      continue;
    }
    if (frames === 0) sr = freq;
    else if (sr !== freq) break;
    frames += 1;
    bytesSum += frameLen;
    pos += frameLen;
  }
  if (frames >= 2 && sr > 0 && bytesSum > 0) {
    const estFrames = Math.round(((fileLen - start) * frames) / bytesSum);
    return Math.round((estFrames * 1024 * 1000) / sr);
  }
  return probeIsoBmffDurationMs(fd, fileLen);
}

function probeOggDurationMs(fd: number, fileLen: number): number | null {
  const head = readExactAt(fd, 0, Math.min(8192, fileLen));
  if (!head || head.length < 36 || head.toString("latin1", 0, 4) !== "OggS") return null;
  const serial = head.readUInt32LE(14);
  const segCount = head[26]!;
  const pktOff = 27 + segCount;
  let sampleRate = 0;
  let preSkip = 0;
  if (pktOff + 19 <= head.length && head.toString("latin1", pktOff, pktOff + 8) === "OpusHead") {
    sampleRate = 48000;
    preSkip = head.readUInt16LE(pktOff + 10);
  } else if (pktOff + 16 <= head.length && head[pktOff] === 1 && head.toString("latin1", pktOff + 1, pktOff + 7) === "vorbis") {
    sampleRate = head.readUInt32LE(pktOff + 12);
  }
  if (sampleRate <= 0) return null;
  const tailLen = Math.min(65536, fileLen);
  const tail = readExactAt(fd, fileLen - tailLen, tailLen);
  if (!tail) return null;
  let lastGranule = -1n;
  for (let i = 0; i + 27 <= tail.length; i += 1) {
    if (tail.toString("latin1", i, i + 4) !== "OggS" || tail[i + 4] !== 0) continue;
    const sc = tail[i + 26]!;
    if (i + 27 + sc > tail.length) continue;
    if (tail.readUInt32LE(i + 14) !== serial) continue;
    const gran = tail.readBigInt64LE(i + 6);
    if (gran > 0n) lastGranule = gran;
  }
  if (lastGranule <= BigInt(preSkip)) return null;
  return Number(((lastGranule - BigInt(preSkip)) * 1000n) / BigInt(sampleRate));
}

function readEbmlVint(buf: Buffer, pos: number, keepMarker: boolean): [bigint, number] | null {
  if (pos >= buf.length) return null;
  const first = buf[pos]!;
  if (first === 0) return null;
  let len = 1;
  while (len <= 8 && (first & (0x80 >> (len - 1))) === 0) len += 1;
  if (len > 8 || pos + len > buf.length) return null;
  let val = BigInt(keepMarker ? first : first & ((1 << (8 - len)) - 1));
  for (let i = 1; i < len; i += 1) {
    val = (val << 8n) | BigInt(buf[pos + i]!);
  }
  return [val, len];
}

function probeEbmlDurationMs(fd: number, fileLen: number): number | null {
  const buf = readExactAt(fd, 0, Math.min(262144, fileLen));
  if (!buf || buf.length < 4 || buf.readUInt32BE(0) !== 0x1a45dfa3) return null;
  let pos = 0;
  const hdrId = readEbmlVint(buf, pos, true);
  if (!hdrId) return null;
  pos += hdrId[1];
  const hdrSz = readEbmlVint(buf, pos, false);
  if (!hdrSz) return null;
  pos += hdrSz[1] + Number(hdrSz[0]);
  while (pos < buf.length) {
    const idRes = readEbmlVint(buf, pos, true);
    if (!idRes) break;
    pos += idRes[1];
    const szRes = readEbmlVint(buf, pos, false);
    if (!szRes) break;
    pos += szRes[1];
    const elemId = Number(idRes[0]);
    const elemSz = Number(szRes[0]);
    if (elemId === 0x18538067) {
      // Segment: continue scanning children inside buffer
      continue;
    }
    if (elemId === 0x1549a966 && pos + elemSz <= buf.length) {
      // Info element
      const infoEnd = pos + elemSz;
      let cpos = pos;
      let scaleNs = 1_000_000;
      let rawDur: number | null = null;
      while (cpos < infoEnd) {
        const cid = readEbmlVint(buf, cpos, true);
        if (!cid) break;
        cpos += cid[1];
        const csz = readEbmlVint(buf, cpos, false);
        if (!csz) break;
        cpos += csz[1];
        const len = Number(csz[0]);
        if (cpos + len > infoEnd) break;
        if (Number(cid[0]) === 0x2ad7b1 && len >= 1 && len <= 8) {
          let v = 0n;
          for (let k = 0; k < len; k += 1) v = (v << 8n) | BigInt(buf[cpos + k]!);
          if (v > 0n) scaleNs = Number(v);
        } else if (Number(cid[0]) === 0x4489 && (len === 4 || len === 8)) {
          rawDur = len === 4 ? buf.readFloatBE(cpos) : buf.readDoubleBE(cpos);
        }
        cpos += len;
      }
      if (rawDur !== null && Number.isFinite(rawDur) && rawDur > 0) {
        const ms = Math.round((rawDur * scaleNs) / 1_000_000);
        return ms > 0 ? ms : null;
      }
      return null;
    }
    if (elemSz <= 0 || pos + elemSz > buf.length) break;
    pos += elemSz;
  }
  return null;
}

function probeMediaDurationMs(filePath: string): number | null {
  const ext = path.extname(filePath).slice(1).toLowerCase();
  if (!SUPPORTED_EXTS.has(ext)) {
    return null;
  }
  let fd: number | null = null;
  try {
    const stat = fs.statSync(filePath);
    if (!stat.isFile() || stat.size <= 0) return null;
    fd = fs.openSync(filePath, "r");
    if (ext === "mp4" || ext === "m4a" || ext === "m4v" || ext === "mov") {
      return probeIsoBmffDurationMs(fd, stat.size);
    }
    if (ext === "mkv" || ext === "webm") {
      return probeEbmlDurationMs(fd, stat.size);
    }
    if (ext === "mp3") {
      return probeMp3DurationMs(fd, stat.size);
    }
    if (ext === "aac") {
      return probeAacAdtsDurationMs(fd, stat.size);
    }
    if (ext === "ogg" || ext === "opus") {
      return probeOggDurationMs(fd, stat.size);
    }
    if (ext === "wav" && stat.size >= 44) {
      const riff = readExactAt(fd, 0, 12);
      if (riff && riff.toString("latin1", 0, 4) === "RIFF" && riff.toString("latin1", 8, 12) === "WAVE") {
        let pos = 12;
        let byteRate = 0;
        let dataSize = 0;
        let guard = 0;
        while (pos + 8 <= stat.size && guard < 64) {
          guard += 1;
          const chdr = readExactAt(fd, pos, 8);
          if (!chdr) break;
          const cid = chdr.toString("latin1", 0, 4);
          const csz = chdr.readUInt32LE(4);
          if (cid === "fmt " && csz >= 16) {
            const fmt = readExactAt(fd, pos + 8, 16);
            if (fmt) byteRate = fmt.readUInt32LE(8);
          } else if (cid === "data") {
            dataSize = Math.min(csz, stat.size - (pos + 8));
            if (byteRate > 0) break;
          }
          pos += 8 + csz + (csz & 1);
        }
        if (byteRate > 0 && dataSize > 0) {
          return Math.round((dataSize * 1000) / byteRate);
        }
      }
    }
    if (ext === "flac" && stat.size >= 42) {
      const start = skipId3v2Tags(fd, stat.size);
      const hdr = readExactAt(fd, start, 42);
      if (hdr && hdr.toString("latin1", 0, 4) === "fLaC" && (hdr[4]! & 0x7f) === 0) {
        const sampleRate = (hdr[18]! << 12) | (hdr[19]! << 4) | (hdr[20]! >> 4);
        const totalSamples =
          (BigInt(hdr[21]! & 0x0f) << 32n) |
          (BigInt(hdr[22]!) << 24n) |
          (BigInt(hdr[23]!) << 16n) |
          (BigInt(hdr[24]!) << 8n) |
          BigInt(hdr[25]!);
        if (sampleRate > 0 && totalSamples > 0n) {
          return Number((totalSamples * 1000n) / BigInt(sampleRate));
        }
      }
    }
    return null;
  } catch {
    return null;
  } finally {
    if (fd !== null) {
      try {
        fs.closeSync(fd);
      } catch {
        // Ignore close error
      }
    }
  }
}

function collectMediaInDir(
  dir: string,
  depth: number,
  maxDepth: number,
  maxFiles: number,
  out: Array<{ path: string; name: string; size: number; duration_ms: number | null }>,
): void {
  if (depth > maxDepth || out.length >= maxFiles) return;
  let entries: fs.Dirent[];
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  entries.sort((a, b) => a.name.localeCompare(b.name));
  for (const entry of entries) {
    if (out.length >= maxFiles) break;
    if (entry.name.startsWith(".") || entry.isSymbolicLink()) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      collectMediaInDir(full, depth + 1, maxDepth, maxFiles, out);
    } else if (entry.isFile()) {
      const ext = path.extname(entry.name).slice(1).toLowerCase();
      if (SUPPORTED_EXTS.has(ext)) {
        try {
          const stat = fs.statSync(full);
          if (stat.size > 0) {
            const canonical = fs.realpathSync(full);
            const duration_ms = probeMediaDurationMs(canonical);
            out.push({ path: canonical, name: entry.name, size: stat.size, duration_ms });
          }
        } catch {
          // Skip unreadable files
        }
      }
    }
  }
}

function parseByteRange(spec: string, totalLen: number): [number, number] | null {
  if (totalLen <= 0 || !spec.startsWith("bytes=")) return null;
  const first = spec.slice(6).split(",")[0]?.trim() ?? "";
  const dashIdx = first.indexOf("-");
  if (dashIdx < 0) return null;
  const startStr = first.slice(0, dashIdx).trim();
  const endStr = first.slice(dashIdx + 1).trim();
  if (startStr === "") {
    const suffixLen = Number.parseInt(endStr, 10);
    if (!Number.isFinite(suffixLen) || suffixLen <= 0) return null;
    const start = Math.max(0, totalLen - suffixLen);
    return [start, totalLen - 1];
  }
  const start = Number.parseInt(startStr, 10);
  if (!Number.isFinite(start) || start < 0 || start >= totalLen) return null;
  if (endStr === "") {
    return [start, totalLen - 1];
  }
  const end = Number.parseInt(endStr, 10);
  if (!Number.isFinite(end) || end < start) return null;
  return [start, Math.min(totalLen - 1, end)];
}

function audoraLocalDevBridge(): Plugin {
  const allowedDevFiles = new Set<string>();
  const allowedDevDirs = new Set<string>();
  let dialogInFlight = false;

  const normalizeKey = (p: string): string =>
    process.platform === "win32" ? p.toLowerCase() : p;

  const isAllowedDevDir = (canonDir: string): boolean => {
    const key = normalizeKey(canonDir);
    if (allowedDevDirs.has(key)) return true;
    for (const dir of allowedDevDirs) {
      if (key.startsWith(dir + path.sep)) return true;
    }
    return false;
  };

  const resolveAllowedDevMediaFile = (rawPath: string): string | null => {
    if (!rawPath || (!path.isAbsolute(rawPath) && !rawPath.startsWith("\\\\"))) return null;
    try {
      const canon = fs.realpathSync(rawPath);
      const stat = fs.statSync(canon);
      if (!stat.isFile()) return null;
      const ext = path.extname(canon).slice(1).toLowerCase();
      if (!SUPPORTED_EXTS.has(ext)) return null;
      const key = normalizeKey(canon);
      if (allowedDevFiles.has(key) || isAllowedDevDir(path.dirname(canon))) {
        return canon;
      }
      return null;
    } catch {
      return null;
    }
  };

  return {
    name: "audora-local-dev-bridge",
    configureServer(server) {
      const allowlistPath = path.resolve(
        server.config.root,
        "node_modules",
        ".vite",
        "audora-dev-allowlist.json",
      );

      const loadPersistedAllowlist = () => {
        try {
          if (!fs.existsSync(allowlistPath)) return;
          const raw = fs.readFileSync(allowlistPath, "utf8");
          const parsed = JSON.parse(raw) as { files?: unknown; dirs?: unknown };
          if (Array.isArray(parsed.files)) {
            for (const entry of parsed.files) {
              if (typeof entry === "string" && (path.isAbsolute(entry) || entry.startsWith("\\\\"))) {
                allowedDevFiles.add(normalizeKey(entry));
              }
            }
          }
          if (Array.isArray(parsed.dirs)) {
            for (const entry of parsed.dirs) {
              if (typeof entry === "string" && (path.isAbsolute(entry) || entry.startsWith("\\\\"))) {
                allowedDevDirs.add(normalizeKey(entry));
              }
            }
          }
        } catch {
          // Ignore malformed or unreadable dev allowlist cache
        }
      };

      const savePersistedAllowlist = () => {
        try {
          fs.mkdirSync(path.dirname(allowlistPath), { recursive: true });
          const tmpPath = `${allowlistPath}.tmp`;
          fs.writeFileSync(
            tmpPath,
            JSON.stringify(
              {
                files: Array.from(allowedDevFiles),
                dirs: Array.from(allowedDevDirs),
              },
              null,
              2,
            ),
            "utf8",
          );
          fs.renameSync(tmpPath, allowlistPath);
        } catch {
          // Ignore cache write errors in read-only environments
        }
      };

      loadPersistedAllowlist();

      server.middlewares.use((req, res, next) => {
        if (!req.url) return next();
        const urlObj = new URL(req.url, "http://127.0.0.1");

        if (urlObj.pathname.startsWith("/__audora_")) {
          const host = String(req.headers.host ?? "").trim();
          const validHost = /^(127\.0\.0\.1|localhost)(:\d+)?$/i.test(host);
          const secFetchSite = String(req.headers["sec-fetch-site"] ?? "").toLowerCase();
          const origin = String(req.headers.origin ?? "").trim();
          const crossSite =
            !validHost ||
            (secFetchSite && secFetchSite !== "same-origin" && secFetchSite !== "none") ||
            origin.toLowerCase() === "null" ||
            (origin && !/^http:\/\/(127\.0\.0\.1|localhost)(:\d+)?$/i.test(origin));
          if (crossSite) {
            res.statusCode = 403;
            res.setHeader("Content-Type", "application/json; charset=utf-8");
            res.end(JSON.stringify({ ok: false, message: "Cross-origin loopback request blocked." }));
            return;
          }
        }

        if (urlObj.pathname === "/__audora_probe_media") {
          const rawPath = urlObj.searchParams.get("path")?.trim() ?? "";
          res.setHeader("Content-Type", "application/json; charset=utf-8");
          const allowed = resolveAllowedDevMediaFile(rawPath);
          if (!allowed) {
            res.statusCode = 404;
            res.end(JSON.stringify({ ok: false, exists: false, duration_ms: null }));
            return;
          }
          const duration_ms = probeMediaDurationMs(allowed);
          res.statusCode = 200;
          res.end(JSON.stringify({ ok: true, exists: true, duration_ms }));
          return;
        }

        if (
          urlObj.pathname === "/__audora_pick_files" ||
          urlObj.pathname === "/__audora_pick_single_file"
        ) {
          if (dialogInFlight) {
            res.statusCode = 409;
            res.setHeader("Content-Type", "application/json; charset=utf-8");
            res.end(
              JSON.stringify({
                ok: false,
                status: "busy",
                message: "A file or folder picker dialog is already open.",
              }),
            );
            return;
          }
          dialogInFlight = true;
          const isSingle = urlObj.pathname === "/__audora_pick_single_file";
          const sendPickedFiles = (rawStdout: string | null) => {
            try {
              res.statusCode = 200;
              res.setHeader("Content-Type", "application/json; charset=utf-8");
              if (!rawStdout) {
                res.end(
                  JSON.stringify(
                    isSingle
                      ? { ok: true, cancelled: true, file: null }
                      : { ok: true, cancelled: true, files: [] },
                  ),
                );
                return;
              }
              const files: Array<{
                path: string;
                name: string;
                size: number;
                duration_ms: number | null;
              }> = [];
              let addedAny = false;
              for (const line of rawStdout.split(/\r?\n/)) {
                const trimmed = line.trim();
                if (!trimmed) continue;
                try {
                  const stat = fs.statSync(trimmed);
                  if (stat.isFile()) {
                    const canonical = fs.realpathSync(trimmed);
                    allowedDevFiles.add(normalizeKey(canonical));
                    addedAny = true;
                    files.push({
                      path: canonical,
                      name: path.basename(canonical),
                      size: stat.size,
                      duration_ms: probeMediaDurationMs(canonical),
                    });
                  }
                } catch {
                  // Skip invalid selection line
                }
              }
              if (addedAny) {
                savePersistedAllowlist();
              }
              if (isSingle) {
                res.end(
                  JSON.stringify({
                    ok: true,
                    cancelled: files.length === 0,
                    file: files[0] ?? null,
                  }),
                );
                return;
              }
              res.end(
                JSON.stringify({
                  ok: true,
                  cancelled: files.length === 0,
                  files,
                }),
              );
            } finally {
              dialogInFlight = false;
            }
          };

          if (process.platform === "darwin") {
            const script = isSingle
              ? 'POSIX path of (choose file with prompt "Locate the moved audio or video file")'
              : [
                  'set chosenFiles to choose file with prompt "Select local audio or video files for Audora" with multiple selections allowed',
                  'set outText to ""',
                  "repeat with f in chosenFiles",
                  "  set outText to outText & (POSIX path of f) & linefeed",
                  "end repeat",
                  "return outText",
                ].join("\n");
            execFile("/usr/bin/osascript", ["-e", script], (err, stdout) => {
              sendPickedFiles(err ? null : stdout);
            });
            return;
          }

          if (process.platform === "win32") {
            const multiFlag = isSingle ? "$false" : "$true";
            execFile(
              "powershell",
              [
                "-NoProfile",
                "-Sta",
                "-Command",
                `[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; $d.Multiselect = ${multiFlag}; if ($d.ShowDialog() -eq 'OK') { $d.FileNames | ForEach-Object { Write-Output $_ } }`,
              ],
              (err, stdout) => {
                sendPickedFiles(err ? null : stdout);
              },
            );
            return;
          }

          const zenityArgs = isSingle
            ? ["--file-selection", "--separator=\n"]
            : ["--file-selection", "--multiple", "--separator=\n"];
          execFile("zenity", zenityArgs, (err, stdout) => {
            sendPickedFiles(err ? null : stdout);
          });
          return;
        }

        if (urlObj.pathname === "/__audora_pick_folder") {
          if (dialogInFlight) {
            res.statusCode = 409;
            res.setHeader("Content-Type", "application/json; charset=utf-8");
            res.end(
              JSON.stringify({
                ok: false,
                status: "busy",
                message: "A file or folder picker dialog is already open.",
              }),
            );
            return;
          }
          dialogInFlight = true;
          const sendPicked = (rawPicked: string | null) => {
            try {
              const cleaned = rawPicked ? rawPicked.trim().replace(/[\\/]+$/, "") : "";
              res.statusCode = 200;
              res.setHeader("Content-Type", "application/json; charset=utf-8");
              if (cleaned && fs.existsSync(cleaned) && fs.statSync(cleaned).isDirectory()) {
                const canonical = fs.realpathSync(cleaned);
                allowedDevDirs.add(normalizeKey(canonical));
                savePersistedAllowlist();
                res.end(JSON.stringify({ ok: true, cancelled: false, path: canonical }));
              } else {
                res.end(JSON.stringify({ ok: true, cancelled: true, path: null }));
              }
            } finally {
              dialogInFlight = false;
            }
          };

          if (process.platform === "darwin") {
            execFile(
              "/usr/bin/osascript",
              [
                "-e",
                'POSIX path of (choose folder with prompt "Select a local media folder for Audora")',
              ],
              (err, stdout) => {
                sendPicked(err ? null : stdout);
              },
            );
            return;
          }

          if (process.platform === "win32") {
            execFile(
              "powershell",
              [
                "-NoProfile",
                "-Sta",
                "-Command",
                "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.FolderBrowserDialog; if ($f.ShowDialog() -eq 'OK') { Write-Output $f.SelectedPath }",
              ],
              (err, stdout) => {
                sendPicked(err ? null : stdout);
              },
            );
            return;
          }

          execFile(
            "zenity",
            ["--file-selection", "--directory", "--title=Select a local media folder for Audora"],
            (err, stdout) => {
              sendPicked(err ? null : stdout);
            },
          );
          return;
        }

        if (urlObj.pathname === "/__audora_scan_folder") {
          const rawPath = urlObj.searchParams.get("path")?.trim() ?? "";
          res.setHeader("Content-Type", "application/json; charset=utf-8");
          if (!rawPath) {
            res.statusCode = 400;
            res.end(JSON.stringify({ ok: false, message: "Folder path is required." }));
            return;
          }
          let canonicalDir: string;
          try {
            canonicalDir = fs.realpathSync(path.resolve(rawPath));
            if (!fs.statSync(canonicalDir).isDirectory()) {
              throw new Error("not dir");
            }
          } catch {
            res.statusCode = 400;
            res.end(
              JSON.stringify({
                ok: false,
                message: "Folder not found or not accessible on this device.",
              }),
            );
            return;
          }
          if (!isAllowedDevDir(canonicalDir)) {
            res.statusCode = 403;
            res.end(
              JSON.stringify({
                ok: false,
                message: "Folder has not been authorized by the user.",
              }),
            );
            return;
          }
          const files: Array<{
            path: string;
            name: string;
            size: number;
            duration_ms: number | null;
          }> = [];
          collectMediaInDir(canonicalDir, 0, 4, 2000, files);
          res.statusCode = 200;
          res.end(
            JSON.stringify({
              ok: true,
              folder_path: canonicalDir,
              folder_name: path.basename(canonicalDir) || canonicalDir,
              files,
            }),
          );
          return;
        }

        if (urlObj.pathname === "/__audora_media") {
          const rawPath = urlObj.searchParams.get("path")?.trim() ?? "";
          const allowed = resolveAllowedDevMediaFile(rawPath);
          if (!allowed) {
            res.statusCode = 404;
            res.end();
            return;
          }
          let stat: fs.Stats;
          try {
            stat = fs.statSync(allowed);
            if (!stat.isFile()) throw new Error("not file");
          } catch {
            res.statusCode = 404;
            res.end();
            return;
          }
          const ext = path.extname(allowed).slice(1).toLowerCase();
          const total = stat.size;
          const contentType = mimeForExt(ext);
          const isHead = req.method === "HEAD";
          const securityHeaders = {
            "Content-Type": contentType,
            "X-Content-Type-Options": "nosniff",
            "Content-Security-Policy": "default-src 'none'",
            "Cache-Control": "no-store",
            "Accept-Ranges": "bytes",
          };
          const range = req.headers.range;
          if (range && total > 0) {
            const parsed = parseByteRange(range, total);
            if (!parsed) {
              res.writeHead(416, {
                ...securityHeaders,
                "Content-Range": `bytes */${total}`,
                "Content-Length": 0,
              });
              res.end();
              return;
            }
            const [start, end] = parsed;
            res.writeHead(206, {
              ...securityHeaders,
              "Content-Range": `bytes ${start}-${end}/${total}`,
              "Content-Length": end - start + 1,
            });
            if (isHead) {
              res.end();
              return;
            }
            const rs = fs.createReadStream(allowed, { start, end });
            rs.on("error", () => res.destroy());
            rs.pipe(res);
            return;
          }
          res.writeHead(200, {
            ...securityHeaders,
            "Content-Length": total,
          });
          if (isHead || total === 0) {
            res.end();
            return;
          }
          const rs = fs.createReadStream(allowed);
          rs.on("error", () => res.destroy());
          rs.pipe(res);
          return;
        }

        if (urlObj.pathname === "/__audora_revoke_folder" && req.method === "POST") {
          const rawPath = urlObj.searchParams.get("path")?.trim() ?? "";
          if (rawPath) {
            try {
              const canon = fs.existsSync(rawPath) ? fs.realpathSync(rawPath) : path.resolve(rawPath);
              if (allowedDevDirs.delete(normalizeKey(canon))) {
                savePersistedAllowlist();
              }
            } catch {
              // Ignore
            }
          }
          res.statusCode = 200;
          res.setHeader("Content-Type", "application/json; charset=utf-8");
          res.end(JSON.stringify({ ok: true }));
          return;
        }

        if (urlObj.pathname === "/__audora_state" && req.method === "DELETE") {
          allowedDevFiles.clear();
          allowedDevDirs.clear();
          try {
            if (fs.existsSync(allowlistPath)) {
              fs.unlinkSync(allowlistPath);
            }
          } catch {
            // Ignore
          }
          res.statusCode = 200;
          res.setHeader("Content-Type", "application/json; charset=utf-8");
          res.end(JSON.stringify({ ok: true, removed_paths: [allowlistPath], bytes_reclaimed: 0 }));
          return;
        }

        // Prevent Vite's SPA HTML fallback from returning `200 OK` on native-only `/__audora_*` routes (`C-3`).
        if (urlObj.pathname.startsWith("/__audora_")) {
          res.statusCode = 404;
          res.setHeader("Content-Type", "application/json; charset=utf-8");
          res.end(
            JSON.stringify({
              ok: false,
              message: "Native desktop endpoint is unavailable in browser dev mode.",
            }),
          );
          return;
        }

        next();
      });
    },
  };
}

export default defineConfig({
  plugins: [react(), audoraLocalDevBridge()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    allowedHosts: ["127.0.0.1", "localhost"],
  },
});
