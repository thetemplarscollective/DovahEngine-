//! Asset provider: read game files (`.nif`, `.dds`, ...) from the
//! user's Data folder — loose files first, then BSA archives.
//!
//! Skyrim paths in ESM/NIF files use backslashes and mixed case
//! (e.g. `Meshes\Architecture\Farmhouse\...`), while BSA archives
//! store forward-slash paths in their own case. All lookups are
//! normalized to lowercase forward-slash paths, so either spelling
//! finds the file. Nothing is copied into the repo: files are read
//! on demand from the data directory the user points at.

use bevy::ecs::prelude::Resource;
use dovah_bsa::BsaArchive;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

/// Normalize a game path for lookup: backslashes to `/`,
/// lowercase, and strip any build-machine prefix.
///
/// M10: some NIFs reference textures by their original
/// build-machine path (`skyrimhd\build\pc\data\textures\...`)
/// instead of the game path (`textures\...`). The game strips
/// everything up to the `data\` dir; do the same, or those
/// textures never resolve — in Riverwood that is the trees, the
/// dirt, the grass, and the plants (13 textures). Game asset
/// paths never contain a `data` segment, so the strip is a no-op
/// for real paths and for every stored BSA path.
fn normalize(path: &str) -> String {
    let lowered = path.replace('\\', "/").to_lowercase();
    match lowered.rfind("/data/") {
        Some(i) => lowered[i + 6..].to_string(),
        None => lowered
            .strip_prefix("data/")
            .unwrap_or(&lowered)
            .to_string(),
    }
}

/// Where a file lives: a loose file on disk, or an entry in one of
/// the open archives (by archive index + stored path).
///
/// `Loose` is currently unused (test-data has no loose mesh/texture
/// files yet) but is the path mod files and unpacked data will take,
/// so the variant stays.
#[allow(dead_code)]
#[derive(Debug, Clone)]
enum Location {
    Loose(PathBuf),
    Archive(usize, String),
}

/// Reads game assets from a Data folder.
#[derive(Resource)]
pub struct AssetProvider {
    data_dir: PathBuf,
    archives: Vec<BsaArchive<File>>,
    index: HashMap<String, Location>,
    /// Archive file names, in open order, for the summary.
    pub archive_names: Vec<String>,
}

impl AssetProvider {
    /// Open every `*.bsa` in `data_dir` and index their contents.
    /// Loose files under `data_dir` are found on demand and take
    /// precedence over archived ones (as in the game).
    pub fn new(data_dir: &Path) -> Self {
        let mut provider = AssetProvider {
            data_dir: data_dir.to_path_buf(),
            archives: Vec::new(),
            index: HashMap::new(),
            archive_names: Vec::new(),
        };
        if let Ok(entries) = fs::read_dir(data_dir) {
            let mut bsa_paths: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension()
                        .map(|e| e.eq_ignore_ascii_case("bsa"))
                        .unwrap_or(false)
                })
                .collect();
            bsa_paths.sort();
            for path in bsa_paths {
                match BsaArchive::open(&path) {
                    Ok(archive) => {
                        let idx = provider.archives.len();
                        for stored in archive.list() {
                            // First archive wins on duplicates (the
                            // game's load order is more subtle; this
                            // is M6's approximation).
                            provider
                                .index
                                .entry(normalize(stored))
                                .or_insert_with(|| Location::Archive(idx, stored.to_string()));
                        }
                        provider.archive_names.push(
                            path.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        );
                        provider.archives.push(archive);
                    }
                    Err(e) => {
                        eprintln!("warning: could not open {}: {e}", path.display());
                    }
                }
            }
        }
        provider
    }

    /// Read a file by game path (e.g. `meshes\architecture\...nif`
    /// or `textures/...dds`). Returns `None` when the file is not in
    /// the data folder or any archive.
    ///
    /// M12: FaceGen NIFs store head/brow/eye diffuse paths relative
    /// to the textures folder (`Actors\Character\Female\FemaleHead.dds`,
    /// no `textures\` prefix), so a lookup that misses is retried
    /// with `textures/` prepended. The retry only fires on a miss,
    /// so real paths are unaffected — and a mesh path can never
    /// match under `textures/` (it starts with `meshes/`).
    pub fn read_file(&mut self, path: &str) -> Option<Vec<u8>> {
        // Loose files win.
        let loose = self.data_dir.join(path.replace('\\', "/"));
        if loose.is_file() {
            return fs::read(loose).ok();
        }
        // Case-insensitive loose lookup is skipped: the archive
        // index below covers the common case, and loose files on a
        // case-sensitive filesystem are expected to match exactly.
        let normalized = normalize(path);
        let mut location = self.index.get(&normalized).cloned();
        if location.is_none() {
            let prefixed = format!("textures/{normalized}");
            location = self.index.get(&prefixed).cloned();
        }
        match location {
            Some(Location::Archive(idx, stored)) => {
                self.archives.get_mut(idx)?.extract(&stored).ok()
            }
            _ => None,
        }
    }

    /// Read a file with LAST-archive-wins semantics (M4): scan the
    /// archives in REVERSE open order and return the first hit. The
    /// game's load order puts Patch.bsa above Interface.bsa, so the
    /// Patch copies of `interface/exported/hudmenu.gfx` and
    /// `interface/fonts_en.swf` are the ones the game loads — but
    /// the provider's index is first-wins (archives are sorted, and
    /// `Skyrim_-_Interface.bsa` sorts before `Skyrim_-_Patch.bsa`),
    /// so `read_file` returns the OLDER Interface copy of both.
    ///
    /// This is the targeted fix for the Ruffle HUD files only; the
    /// general BSA load-order fix (Patch overrides Interface
    /// everywhere) is separate work. The reverse scan lists each
    /// archive's files, so it is slower than the index lookup — call
    /// it at load time, not per frame. Loose files still win, as in
    /// `read_file`.
    pub fn read_file_latest(&mut self, path: &str) -> Option<Vec<u8>> {
        let loose = self.data_dir.join(path.replace('\\', "/"));
        if loose.is_file() {
            return fs::read(loose).ok();
        }
        let normalized = normalize(path);
        for idx in (0..self.archives.len()).rev() {
            let stored = {
                let archive = &self.archives[idx];
                archive
                    .list()
                    .into_iter()
                    .find(|stored| normalize(stored) == normalized)
                    .map(|s| s.to_string())
            };
            if let Some(stored) = stored {
                if let Some(bytes) = self
                    .archives
                    .get_mut(idx)
                    .and_then(|a| a.extract(&stored).ok())
                {
                    return Some(bytes);
                }
            }
        }
        None
    }

    /// The data folder this provider reads from (M4: the Ruffle
    /// HUD uses it as the movie's base path for relative imports).
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// Number of files indexed across all archives.
    pub fn indexed_files(&self) -> usize {
        self.index.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_unifies_separators_and_case() {
        assert_eq!(
            normalize("Meshes\\Architecture\\Farmhouse\\Door01.nif"),
            "meshes/architecture/farmhouse/door01.nif"
        );
        assert_eq!(
            normalize("textures/sky/clouds.dds"),
            "textures/sky/clouds.dds"
        );
    }

    #[test]
    fn normalize_strips_build_machine_prefix() {
        // Real paths from Riverwood NIFs (M10): the trees, dirt,
        // and plants all carry the build-machine prefix.
        assert_eq!(
            normalize(
                "skyrimhd\\build\\pc\\data\\textures\\landscape\\trees\\TreePineForestBarkComp.dds"
            ),
            "textures/landscape/trees/treepineforestbarkcomp.dds"
        );
        assert_eq!(
            normalize("skyrimhd/build/pc/data/textures/landscape/Dirt01.dds"),
            "textures/landscape/dirt01.dds"
        );
        // A path with no prefix is untouched.
        assert_eq!(
            normalize("textures\\architecture\\farmhouse\\Ivy01.dds"),
            "textures/architecture/farmhouse/ivy01.dds"
        );
    }

    #[test]
    fn empty_dir_opens_with_no_archives() {
        let dir = std::env::temp_dir().join("dovah-app-test-empty-dir");
        let _ = fs::create_dir_all(&dir);
        let provider = AssetProvider::new(&dir);
        assert!(provider.archive_names.is_empty());
        assert_eq!(provider.indexed_files(), 0);
        let _ = fs::remove_dir(&dir);
    }
}
