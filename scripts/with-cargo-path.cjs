const { spawnSync } = require("node:child_process");
const { existsSync } = require("node:fs");
const { join, delimiter } = require("node:path");

const cargoBin = join(process.env.USERPROFILE || "", ".cargo", "bin");
const env = { ...process.env };
const pathKey = Object.keys(env).find((key) => key.toLowerCase() === "path") || "PATH";
const currentPath = env[pathKey] || "";

if (existsSync(join(cargoBin, "cargo.exe")) && !currentPath.includes(cargoBin)) {
  env[pathKey] = `${cargoBin}${delimiter}${currentPath}`;
}

const [command, ...args] = process.argv.slice(2);

if (!command) {
  console.error("Usage: node scripts/with-cargo-path.cjs <command> [...args]");
  process.exit(1);
}

const result = spawnSync(command, args, {
  env,
  stdio: "inherit",
  shell: true,
});

process.exit(result.status ?? 1);
