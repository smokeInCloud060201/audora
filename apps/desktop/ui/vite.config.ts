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

function collectMediaInDir(
  dir: string,
  depth: number,
  maxDepth: number,
  maxFiles: number,
  out: Array<{ path: string; name: string; size: number }>,
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
            out.push({ path: full, name: entry.name, size: stat.size });
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

        if (urlObj.pathname === "/__audora_pick_folder") {
          const sendPicked = (rawPicked: string | null) => {
            const cleaned = rawPicked ? rawPicked.trim().replace(/[\\/]+$/, "") : "";
            res.statusCode = 200;
            res.setHeader("Content-Type", "application/json; charset=utf-8");
            if (cleaned && fs.existsSync(cleaned) && fs.statSync(cleaned).isDirectory()) {
              res.end(JSON.stringify({ ok: true, cancelled: false, path: cleaned }));
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
          const files: Array<{ path: string; name: string; size: number }> = [];
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
