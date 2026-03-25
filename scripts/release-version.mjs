import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const rawVersion = process.argv[2];

if (!rawVersion) {
  console.error("usage: npm run release:version -- <version>");
  process.exit(1);
}

const version = rawVersion.replace(/^v/, "");
const semverPattern = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

if (!semverPattern.test(version)) {
  console.error(`invalid version: ${rawVersion}`);
  process.exit(1);
}

function readJson(relativePath) {
  return JSON.parse(fs.readFileSync(path.join(root, relativePath), "utf8"));
}

function writeJson(relativePath, value) {
  fs.writeFileSync(path.join(root, relativePath), `${JSON.stringify(value, null, 2)}\n`);
}

function readText(relativePath) {
  return fs.readFileSync(path.join(root, relativePath), "utf8");
}

function writeText(relativePath, value) {
  fs.writeFileSync(path.join(root, relativePath), value);
}

function updateCargoPackageVersion(cargoToml, nextVersion) {
  const lines = cargoToml.split(/\r?\n/);
  let inPackageSection = false;
  let updated = false;

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const trimmed = line.trim();

    if (/^\[.*\]$/.test(trimmed)) {
      inPackageSection = trimmed === "[package]";
      continue;
    }

    if (!inPackageSection) {
      continue;
    }

    if (/^version\s*=/.test(trimmed)) {
      lines[index] = line.replace(/^(\s*version\s*=\s*)"[^"]*"/, `$1"${nextVersion}"`);
      updated = true;
      break;
    }
  }

  if (!updated) {
    throw new Error("could not find version in [package] section of src-tauri/Cargo.toml");
  }

  const trailingNewline = cargoToml.endsWith("\n") ? "\n" : "";
  return `${lines.join("\n")}${trailingNewline}`;
}

const packageJson = readJson("package.json");
packageJson.version = version;
writeJson("package.json", packageJson);

const packageLock = readJson("package-lock.json");
packageLock.version = version;
if (packageLock.packages?.[""]) {
  packageLock.packages[""].version = version;
}
writeJson("package-lock.json", packageLock);

const tauriConfig = readJson("src-tauri/tauri.conf.json");
tauriConfig.version = version;
writeJson("src-tauri/tauri.conf.json", tauriConfig);

const cargoToml = readText("src-tauri/Cargo.toml");
const nextCargoToml = updateCargoPackageVersion(cargoToml, version);

writeText("src-tauri/Cargo.toml", nextCargoToml);

console.log(`release-version: updated package.json, package-lock.json, src-tauri/Cargo.toml, and src-tauri/tauri.conf.json to ${version}`);
