
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

// Get current directory
const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const rootDir = path.resolve(__dirname, '..');

const packageJsonPath = path.join(rootDir, 'package.json');
const tauriConfPath = path.join(rootDir, 'src-tauri', 'tauri.conf.json');
const cargoTomlPath = path.join(rootDir, 'src-tauri', 'Cargo.toml');

// Read package.json
const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8'));
const version = packageJson.version;

console.log(`[Version Sync] Target version: ${version}`);

// Update tauri.conf.json
if (fs.existsSync(tauriConfPath)) {
    const tauriConf = JSON.parse(fs.readFileSync(tauriConfPath, 'utf8'));
    if (tauriConf.version !== version) {
        console.log(`[Version Sync] Updating tauri.conf.json from ${tauriConf.version} to ${version}`);
        tauriConf.version = version;
        fs.writeFileSync(tauriConfPath, JSON.stringify(tauriConf, null, 2));
    } else {
        console.log(`[Version Sync] tauri.conf.json is already up to date.`);
    }
} else {
    console.error(`[Version Sync] Error: tauri.conf.json not found at ${tauriConfPath}`);
}

// Update Cargo.toml
if (fs.existsSync(cargoTomlPath)) {
    let cargoToml = fs.readFileSync(cargoTomlPath, 'utf8');
    
    // Regex to find version = "x.y.z" under [package]
    // We assume [package] is near the top and the version key belongs to it.
    // A simple regex might be risky if version is used elsewhere, but in Cargo.toml [package] version is standard.
    // We'll look for `version = "..."` closely following `[package]` or just replace the first occurrence which is 99% correct for Cargo.toml.
    
    // Better regex: look for `version = "..."`
    const versionRegex = /^version\s*=\s*"(.*)"/m;
    const match = cargoToml.match(versionRegex);
    
    if (match) {
        const currentCargoVersion = match[1];
        if (currentCargoVersion !== version) {
            console.log(`[Version Sync] Updating Cargo.toml from ${currentCargoVersion} to ${version}`);
            cargoToml = cargoToml.replace(versionRegex, `version = "${version}"`);
            fs.writeFileSync(cargoTomlPath, cargoToml);
        } else {
            console.log(`[Version Sync] Cargo.toml is already up to date.`);
        }
    } else {
         console.error(`[Version Sync] Error: Could not find version key in Cargo.toml`);
    }
} else {
    console.error(`[Version Sync] Error: Cargo.toml not found at ${cargoTomlPath}`);
}
