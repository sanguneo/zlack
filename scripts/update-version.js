
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const rootDir = path.resolve(__dirname, '..');

const packageJsonPath = path.join(rootDir, 'package.json');
const tauriConfPath = path.join(rootDir, 'src-tauri', 'tauri.conf.json');
const cargoTomlPath = path.join(rootDir, 'src-tauri', 'Cargo.toml');

function fail(message) {
    console.error(`[Version Sync] Error: ${message}`);
    process.exit(1);
}

function packageVersion(cargoToml) {
    const section = cargoToml.match(/^\[package\][^\[]*/m);
    if (!section) {
        return null;
    }
    const version = section[0].match(/^version\s*=\s*"([^"]*)"/m);
    return version ? version[1] : null;
}

const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8'));
const version = packageJson.version;

if (typeof version !== 'string' || version.length === 0) {
    fail('package.json has no version');
}

console.log(`[Version Sync] Target version: ${version}`);

if (!fs.existsSync(tauriConfPath)) {
    fail(`tauri.conf.json not found at ${tauriConfPath}`);
}

const tauriConf = JSON.parse(fs.readFileSync(tauriConfPath, 'utf8'));
if (tauriConf.version !== version) {
    console.log(`[Version Sync] Updating tauri.conf.json from ${tauriConf.version} to ${version}`);
    tauriConf.version = version;
    fs.writeFileSync(tauriConfPath, JSON.stringify(tauriConf, null, 2));
} else {
    console.log(`[Version Sync] tauri.conf.json is already up to date.`);
}

const writtenTauriConf = JSON.parse(fs.readFileSync(tauriConfPath, 'utf8'));
if (writtenTauriConf.version !== version) {
    fail(`tauri.conf.json version is ${writtenTauriConf.version}, expected ${version}`);
}

if (!fs.existsSync(cargoTomlPath)) {
    fail(`Cargo.toml not found at ${cargoTomlPath}`);
}

let cargoToml = fs.readFileSync(cargoTomlPath, 'utf8');
const currentCargoVersion = packageVersion(cargoToml);
if (currentCargoVersion === null) {
    fail('Could not find version key in the [package] section of Cargo.toml');
}

if (currentCargoVersion !== version) {
    console.log(`[Version Sync] Updating Cargo.toml from ${currentCargoVersion} to ${version}`);
    const sectionPattern = /^\[package\][^\[]*/m;
    const section = cargoToml.match(sectionPattern);
    const updatedSection = section[0].replace(/^version\s*=\s*"[^"]*"/m, `version = "${version}"`);
    if (packageVersion(updatedSection) !== version) {
        fail('Could not update the version key in the [package] section of Cargo.toml');
    }
    cargoToml = cargoToml.replace(sectionPattern, updatedSection);
    fs.writeFileSync(cargoTomlPath, cargoToml);
} else {
    console.log(`[Version Sync] Cargo.toml is already up to date.`);
}

const writtenCargoVersion = packageVersion(fs.readFileSync(cargoTomlPath, 'utf8'));
if (writtenCargoVersion !== version) {
    fail(`Cargo.toml [package] version is ${writtenCargoVersion}, expected ${version}`);
}
