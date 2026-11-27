import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const desktopDir = path.resolve(__dirname, "..");
const repoRoot = path.resolve(desktopDir, "..");
const binariesDir = path.join(desktopDir, "src-tauri", "binaries");

const candidates = [
  process.env.RELINER_BACKEND_EXE,
  path.join(repoRoot, "target", "x86_64-pc-windows-msvc", "release", "reliner.exe"),
  path.join(repoRoot, "target", "x86_64-pc-windows-gnu", "release", "reliner.exe"),
  path.join(repoRoot, "target", "release", "reliner.exe"),
  path.join(repoRoot, "target", "debug", "reliner.exe"),
].filter(Boolean);

const source = candidates.find((candidate) => fs.existsSync(candidate));
if (!source) {
  console.error(
    "[sync-backend] 未找到 reliner.exe，请先构建后端或设置 RELINER_BACKEND_EXE。检查路径：\n" +
      candidates.join("\n")
  );
  process.exit(1);
}

fs.mkdirSync(binariesDir, { recursive: true });
const triples = ["x86_64-pc-windows-msvc", "x86_64-pc-windows-gnu"];
for (const triple of triples) {
  const target = path.join(binariesDir, `reliner-${triple}.exe`);
  const srcStat = fs.statSync(source);
  const needsCopy =
    !fs.existsSync(target) ||
    fs.statSync(target).size !== srcStat.size ||
    fs.statSync(target).mtimeMs < srcStat.mtimeMs;
  if (needsCopy) {
    fs.copyFileSync(source, target);
    console.log(`[sync-backend] Copied ${source} -> ${target}`);
  }
}
