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
  let sidxFallbackMs: number | null = null;
  let guard = 0;

  while (pos < fileLen && guard < 256) {
    guard += 1;
    const box = readMp4BoxHeader(fd, pos, fileLen);
    if (!box) break;

    if (box.type === "moov") {
      const moovEnd = pos + box.size;
      let cpos = pos + box.hdrLen;
      let cguard = 0;
      while (cpos < moovEnd && cguard < 128) {
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
    } else if (box.type === "sidx" && sidxFallbackMs === null) {
      const payloadLen = box.size - box.hdrLen;
      if (payloadLen >= 24 && payloadLen <= 2_097_152) {
        const sidx = readExactAt(fd, pos + box.hdrLen, payloadLen);
        if (sidx) {
          const ver = sidx[0];
          const timescale = BigInt(sidx.readUInt32BE(8));
          const entriesOffset = ver === 0 ? 20 : 28;
          if (timescale > 0n && sidx.length >= entriesOffset + 4) {
            const count = sidx.readUInt16BE(entriesOffset + 2);
            const tableStart = entriesOffset + 4;
            if (sidx.length >= tableStart + count * 12) {
              let sum = 0n;
              for (let i = 0; i < count; i += 1) {
                sum += BigInt(sidx.readUInt32BE(tableStart + i * 12 + 4));
              }
              if (sum > 0n) {
                const ms = Number((sum * 1000n) / timescale);
                if (Number.isFinite(ms) && ms > 0) sidxFallbackMs = ms;
              }
            }
          }
        }
      }
    }

    pos += box.size;
  }

  return sidxFallbackMs;
}

function probeMediaDurationMs(filePath: string): number | null {
  const ext = path.extname(filePath).slice(1).toLowerCase();
  if (!["mp4", "m4a", "m4v", "mov", "wav", "flac"].includes(ext)) {
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
      const hdr = readExactAt(fd, 0, 42);
      if (hdr && hdr.toString("latin1", 0, 4) === "fLaC" && (hdr[4] & 0x7f) === 0) {
        const sampleRate = (hdr[18] << 12) | (hdr[19] << 4) | (hdr[20] >> 4);
        const totalSamples =
          (BigInt(hdr[21] & 0x0f) << 32n) |
          (BigInt(hdr[22]) << 24n) |
          (BigInt(hdr[23]) << 16n) |
          (BigInt(hdr[24]) << 8n) |
          BigInt(hdr[25]);
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

function audoraLocalDevBridge(): Plugin {
  return {
    name: "audora-local-dev-bridge",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        if (!req.url) return next();
        const urlObj = new URL(req.url, "http://localhost");

        if (urlObj.pathname === "/__audora_probe_media") {
          const rawPath = urlObj.searchParams.get("path")?.trim() ?? "";
          res.setHeader("Content-Type", "application/json; charset=utf-8");
          if (!rawPath || !path.isAbsolute(rawPath)) {
            res.statusCode = 404;
            res.end(JSON.stringify({ ok: false, duration_ms: null }));
            return;
          }
          const duration_ms = probeMediaDurationMs(rawPath);
          res.statusCode = 200;
          res.end(JSON.stringify({ ok: true, duration_ms }));
          return;
        }

        if (urlObj.pathname === "/__audora_pick_files") {
          const sendPickedFiles = (rawStdout: string | null) => {
            res.statusCode = 200;
            res.setHeader("Content-Type", "application/json; charset=utf-8");
            if (!rawStdout) {
              res.end(JSON.stringify({ ok: true, cancelled: true, files: [] }));
              return;
            }
            const files: Array<{
              path: string;
              name: string;
              size: number;
              duration_ms: number | null;
            }> = [];
            for (const line of rawStdout.split(/\r?\n/)) {
              const trimmed = line.trim();
              if (!trimmed) continue;
              try {
                const stat = fs.statSync(trimmed);
                if (stat.isFile()) {
                  const canonical = fs.realpathSync(trimmed);
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
            res.end(
              JSON.stringify({
                ok: true,
                cancelled: files.length === 0,
                files,
              }),
            );
          };

          if (process.platform === "darwin") {
            const script = [
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
            execFile(
              "powershell",
              [
                "-NoProfile",
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; $d.Multiselect = $true; if ($d.ShowDialog() -eq 'OK') { $d.FileNames | ForEach-Object { Write-Output $_ } }",
              ],
              (err, stdout) => {
                sendPickedFiles(err ? null : stdout);
              },
            );
            return;
          }

          execFile(
            "zenity",
            ["--file-selection", "--multiple", "--separator=\n"],
            (err, stdout) => {
              sendPickedFiles(err ? null : stdout);
            },
          );
          return;
        }

        if (urlObj.pathname === "/__audora_pick_folder") {
          const sendPicked = (rawPicked: string | null) => {
            const cleaned = rawPicked ? rawPicked.trim().replace(/[\\/]+$/, "") : "";
            res.statusCode = 200;
            res.setHeader("Content-Type", "application/json; charset=utf-8");
            if (cleaned && fs.existsSync(cleaned) && fs.statSync(cleaned).isDirectory()) {
              const canonical = fs.realpathSync(cleaned);
              res.end(JSON.stringify({ ok: true, cancelled: false, path: canonical }));
            } else {
              res.end(JSON.stringify({ ok: true, cancelled: true, path: null }));
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
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.FolderBrowserDialog; if ($f.ShowDialog() -eq 'OK') { Write-Output $f.SelectedPath }",
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
          if (!rawPath) {
            res.statusCode = 400;
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify({ ok: false, message: "Folder path is required." }));
            return;
          }
          const target = path.resolve(rawPath);
          try {
            if (!fs.statSync(target).isDirectory()) {
              throw new Error("not dir");
            }
          } catch {
            res.statusCode = 400;
            res.setHeader("Content-Type", "application/json");
            res.end(
              JSON.stringify({
                ok: false,
                message: "Folder not found or not accessible on this device.",
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
          collectMediaInDir(target, 0, 4, 2000, files);
          res.statusCode = 200;
          res.setHeader("Content-Type", "application/json; charset=utf-8");
          res.end(
            JSON.stringify({
              ok: true,
              folder_path: target,
              folder_name: path.basename(target) || target,
              files,
            }),
          );
          return;
        }

        if (urlObj.pathname === "/__audora_media") {
          const rawPath = urlObj.searchParams.get("path")?.trim() ?? "";
          const ext = path.extname(rawPath).slice(1).toLowerCase();
          if (!rawPath || !path.isAbsolute(rawPath) || !SUPPORTED_EXTS.has(ext)) {
            res.statusCode = 404;
            res.end();
            return;
          }
          let stat: fs.Stats;
          try {
            stat = fs.statSync(rawPath);
            if (!stat.isFile()) throw new Error("not file");
          } catch {
            res.statusCode = 404;
            res.end();
            return;
          }
          const total = stat.size;
          const contentType = mimeForExt(ext);
          const range = req.headers.range;
          if (range && range.startsWith("bytes=") && total > 0) {
            const [sStr, eStr] = range.slice(6).split("-");
            const start = Math.max(0, Number.parseInt(sStr ?? "0", 10) || 0);
            const end = eStr
              ? Math.min(total - 1, Number.parseInt(eStr, 10) || total - 1)
              : total - 1;
            if (start < total && start <= end) {
              res.writeHead(206, {
                "Content-Type": contentType,
                "Accept-Ranges": "bytes",
                "Content-Range": `bytes ${start}-${end}/${total}`,
                "Content-Length": end - start + 1,
              });
              fs.createReadStream(rawPath, { start, end }).pipe(res);
              return;
            }
          }
          res.writeHead(200, {
            "Content-Type": contentType,
            "Accept-Ranges": "bytes",
            "Content-Length": total,
          });
          fs.createReadStream(rawPath).pipe(res);
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
  },
});
