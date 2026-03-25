import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const expectedEndpoint = "https://github.com/Pixlox/copi/releases/latest/download/latest.json";
const placeholderPubkey = "REPLACE_WITH_TAURI_UPDATER_PUBLIC_KEY";

function readJson(relativePath) {
  return JSON.parse(fs.readFileSync(path.join(root, relativePath), "utf8"));
}

function readText(relativePath) {
  return fs.readFileSync(path.join(root, relativePath), "utf8");
}

function writeFailure(message) {
  console.error(`release-check: ${message}`);
}

function getCargoVersion(cargoToml) {
  const match = cargoToml.match(/^\[package\][\s\S]*?^version = "([^"]+)"/m);
  if (!match) {
    throw new Error("could not find the package version in src-tauri/Cargo.toml");
  }

  return match[1];
}

const packageJson = readJson("package.json");
const packageLock = readJson("package-lock.json");
const tauriConfig = readJson("src-tauri/tauri.conf.json");
const cargoToml = readText("src-tauri/Cargo.toml");

const versions = {
  packageJson: packageJson.version,
  packageLock: packageLock.version,
  tauri: tauriConfig.version,
  cargo: getCargoVersion(cargoToml),
};

const errors = [];

if (new Set(Object.values(versions)).size !== 1) {
  errors.push(
    `version mismatch detected: package.json=${versions.packageJson}, package-lock.json=${versions.packageLock}, tauri.conf.json=${versions.tauri}, Cargo.toml=${versions.cargo}`
  );
}

const tagName = process.env.RELEASE_TAG ?? process.env.GITHUB_REF_NAME ?? "";
if (tagName.startsWith("v")) {
  const tagVersion = tagName.slice(1);
  if (tagVersion !== versions.packageJson) {
    errors.push(`git tag ${tagName} does not match app version ${versions.packageJson}`);
  }
}

const updaterConfig = tauriConfig.plugins?.updater;
if (!updaterConfig) {
  errors.push("src-tauri/tauri.conf.json is missing plugins.updater");
} else {
  if (!updaterConfig.pubkey || updaterConfig.pubkey === placeholderPubkey) {
    errors.push("replace the updater public key placeholder in src-tauri/tauri.conf.json");
  }

  const endpoint = updaterConfig.endpoints?.[0];
  if (endpoint !== expectedEndpoint) {
    errors.push(`unexpected updater endpoint: ${endpoint ?? "missing"}`);
  }
}

if (errors.length > 0) {
  for (const error of errors) {
    writeFailure(error);
  }
  process.exit(1);
}

console.log(
  `release-check: OK (version ${versions.packageJson}, updater endpoint ${expectedEndpoint})`
);
