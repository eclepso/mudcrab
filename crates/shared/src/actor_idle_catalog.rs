//! Optional, typed idle bindings produced from effective RACE authoring and actor bodies.
//! The catalog carries full runtime keys; consumers never infer a rig from a species name.

use crate::{form_key::NamedFormKey, pack_identity::PackIdentity};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PATH: &str = "meshes/actor-idles/catalog.json";
pub const VERSION: u32 = 2;

/// Named wire representation; numeric keys remain inside authenticated pack members.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActorIdleCatalogFile {
    pub version: u32,
    pub pack_identity: String,
    pub bindings: Vec<ActorIdleBindingFile>,
    pub unresolved: Vec<UnresolvedIdleFile>,
    #[serde(default)]
    pub clip_issues: Vec<IdleClipIssueFile>,
    #[serde(default)]
    pub discovery_issues: Vec<String>,
    #[serde(default)]
    pub excluded: Vec<UnresolvedIdleFile>,
}

/// Named wire representation; numeric keys remain inside authenticated pack members.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorIdleBindingFile {
    pub race_id: NamedFormKey,
    pub sex: String,
    pub body_scene: String,
    pub project: String,
    pub skeleton: String,
    pub race_skeleton_nif: String,
    pub bound_tracks: usize,
    pub moving_bound_tracks: usize,
    pub skipped_tracks: Vec<String>,
    pub source: String,
    pub scene: String,
    pub clips: BTreeMap<String, ActorIdleClip>,
}

/// Named wire representation; numeric keys remain inside authenticated pack members.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdleClipIssueFile {
    pub race_id: NamedFormKey,
    pub sex: String,
    pub body_scene: String,
    pub source: String,
    pub reason: String,
    pub not_applicable: bool,
    pub identified_as_idle: bool,
}

/// Named wire representation; numeric keys remain inside authenticated pack members.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedIdleFile {
    pub race_id: NamedFormKey,
    pub sex: String,
    pub body_scene: String,
    pub reason: String,
}

impl ActorIdleCatalogFile {
    /// Authenticate the producing pack before interpreting any portable record identity.
    pub fn validate_for(&self, identity: &PackIdentity) -> color_eyre::Result<()> {
        color_eyre::eyre::ensure!(
            self.version == VERSION,
            "unsupported actor idle catalog; convert again"
        );
        color_eyre::eyre::ensure!(
            self.pack_identity == identity.digest_hex(),
            "actor idle catalog belongs to another conversion; convert again"
        );
        let mut seen = BTreeSet::new();
        for row in &self.bindings {
            let key = identity
                .table()
                .intern(&row.race_id)?
                .ok_or_else(|| color_eyre::eyre::eyre!("idle binding has no race identity"))?;
            color_eyre::eyre::ensure!(
                seen.insert((key, &row.sex, &row.body_scene)),
                "duplicate actor idle binding"
            );
            // Metadata validation is deliberately independent of identity validation above.
            // The zero here never leaves this one-row metadata probe or becomes a handle.
            let mut metadata = serde_json::to_value(row)?;
            metadata["race_id"] = serde_json::json!(0);
            let probe = ActorIdleCatalog {
                version: VERSION,
                bindings: vec![serde_json::from_value(metadata)?],
                ..Default::default()
            };
            probe.validate().map_err(color_eyre::eyre::Error::msg)?;
        }
        for row in self.unresolved.iter().chain(&self.excluded) {
            identity.table().intern(&row.race_id)?;
        }
        for row in &self.clip_issues {
            identity.table().intern(&row.race_id)?;
        }
        Ok(())
    }

    /// Resolve representable runtime races and retain valid neighboring bindings.
    pub fn to_runtime(&self, identity: &PackIdentity) -> color_eyre::Result<ActorIdleCatalog> {
        self.validate_for(identity)?;
        let mut value = serde_json::to_value(self)?;
        value
            .as_object_mut()
            .expect("catalog object")
            .remove("pack_identity");
        let mut unavailable = 0usize;
        for group in ["bindings", "unresolved", "clip_issues", "excluded"] {
            let Some(rows) = value
                .get_mut(group)
                .and_then(serde_json::Value::as_array_mut)
            else {
                continue;
            };
            let mut kept = Vec::with_capacity(rows.len());
            for mut row in rows.drain(..) {
                let named: NamedFormKey = serde_json::from_value(row["race_id"].clone())?;
                let result = identity
                    .table()
                    .intern(&named)
                    .map_err(color_eyre::eyre::Error::new)
                    .and_then(|key| key.map_or(Ok(0), |key| identity.runtime_id(key)));
                match result {
                    Ok(runtime) => {
                        row["race_id"] = serde_json::json!(runtime);
                        kept.push(row);
                    }
                    Err(_) => unavailable += 1,
                }
            }
            *rows = kept;
        }
        let mut catalog: ActorIdleCatalog = serde_json::from_value(value)?;
        if unavailable != 0 {
            catalog.discovery_issues.push(format!("{unavailable} actor idle entries require runtime identity slots unavailable before step 4"));
        }
        catalog.validate().map_err(color_eyre::eyre::Error::msg)?;
        Ok(catalog)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActorIdleCatalog {
    pub version: u32,
    pub bindings: Vec<ActorIdleBinding>,
    pub unresolved: Vec<UnresolvedIdle>,
    #[serde(default)]
    pub clip_issues: Vec<IdleClipIssue>,
    #[serde(default)]
    pub discovery_issues: Vec<String>,
    #[serde(default)]
    pub excluded: Vec<UnresolvedIdle>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorIdleBinding {
    pub race_id: u32,
    pub sex: String,
    pub body_scene: String,
    pub project: String,
    pub skeleton: String,
    pub race_skeleton_nif: String,
    pub bound_tracks: usize,
    pub moving_bound_tracks: usize,
    pub skipped_tracks: Vec<String>,
    pub source: String,
    pub scene: String,
    pub clips: BTreeMap<String, ActorIdleClip>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorIdleClip {
    pub scene: String,
    pub frame_count: u32,
    pub frames_per_second: f32,
    pub motion_frame: Option<u32>,
    pub bound_tracks: usize,
    pub moving_bound_tracks: usize,
    pub skipped_tracks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdleClipIssue {
    pub race_id: u32,
    pub sex: String,
    pub body_scene: String,
    pub source: String,
    pub reason: String,
    pub not_applicable: bool,
    pub identified_as_idle: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedIdle {
    pub race_id: u32,
    pub sex: String,
    pub body_scene: String,
    pub reason: String,
}

pub fn canonical_mesh_key(key: &str, extension: &str) -> bool {
    key.starts_with("meshes/")
        && key.ends_with(extension)
        && key == key.to_ascii_lowercase()
        && key.split('/').all(|part| {
            !part.is_empty()
                && !matches!(part, "." | ".." | "%2e" | ".%2e" | "%2e." | "%2e%2e")
                && !part.contains(['\\', ':', '\0', '#', '?'])
                && !part.chars().any(char::is_control)
        })
}

/// The same authored body key for catalog publication and runtime lookup.
/// Decode once, collapse authoring prefixes and separators, and reject escapes.
pub fn canonical_body_path(input: &str, extension: &str) -> Result<String, &'static str> {
    if !matches!(extension, "nif" | "glb") {
        return Err("unsupported body output extension");
    }
    let bytes = input.trim().as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes
                .get(index + 1..index + 3)
                .and_then(|value| std::str::from_utf8(value).ok())
                .and_then(|value| u8::from_str_radix(value, 16).ok())
                .ok_or("invalid body percent encoding")?;
            decoded.push(hex);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).map_err(|_| "invalid body UTF-8")?;
    let normalized = decoded.replace('\\', "/");
    if normalized.starts_with('/')
        || normalized.contains(':')
        || normalized.chars().any(char::is_control)
    {
        return Err("absolute or invalid body path");
    }
    let parts: Vec<_> = normalized
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.contains(&"..") {
        return Err("body path contains traversal");
    }
    let start = parts
        .iter()
        .rposition(|part| part.eq_ignore_ascii_case("meshes"))
        .map_or(0, |index| index + 1);
    if start == parts.len() {
        return Err("body path has no file name");
    }
    let mut relative = std::path::PathBuf::from(parts[start..].join("/"));
    if relative
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            !value.eq_ignore_ascii_case("nif") && !value.eq_ignore_ascii_case("glb")
        })
    {
        return Err("unsupported body extension");
    }
    relative.set_extension(extension);
    let key = format!("meshes/{}", relative.to_string_lossy()).to_ascii_lowercase();
    if !canonical_mesh_key(&key, &format!(".{extension}")) {
        return Err("invalid canonical body path");
    }
    Ok(key)
}

/// Check one published source/clip pair with the same rules the catalog applies.
pub fn validate_clip(source: &str, clip: &ActorIdleClip) -> Result<(), &'static str> {
    if !canonical_mesh_key(source, ".hkx")
        || !clip.scene.starts_with("meshes/actor-idles/clips/")
        || !canonical_mesh_key(&clip.scene, ".glb")
        || clip.bound_tracks == 0
        || clip.frame_count == 0
        || !clip.frames_per_second.is_finite()
        || clip.frames_per_second <= 0.0
        || clip
            .motion_frame
            .is_some_and(|frame| frame == 0 || frame >= clip.frame_count)
        || clip.moving_bound_tracks > clip.bound_tracks
    {
        return Err("invalid actor idle variant");
    }
    Ok(())
}

impl ActorIdleCatalog {
    /// Encode the runtime helper's identities as explicit names rather than JSON numbers.
    /// Cap-free converter producers construct `ActorIdleCatalogFile` from FormKeys directly.
    pub fn to_file(&self, identity: &PackIdentity) -> color_eyre::Result<ActorIdleCatalogFile> {
        self.validate().map_err(color_eyre::eyre::Error::msg)?;
        let mut value = serde_json::to_value(self)?;
        value["pack_identity"] = serde_json::json!(identity.digest_hex());
        for group in ["bindings", "unresolved", "clip_issues", "excluded"] {
            let Some(rows) = value
                .get_mut(group)
                .and_then(serde_json::Value::as_array_mut)
            else {
                continue;
            };
            for row in rows {
                let raw = row["race_id"]
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| color_eyre::eyre::eyre!("invalid runtime race handle"))?;
                row["race_id"] = match identity.runtime_key(raw)? {
                    Some(key) => serde_json::to_value(identity.table().named(key)?)?,
                    None => serde_json::Value::Null,
                };
            }
        }
        let file: ActorIdleCatalogFile = serde_json::from_value(value)?;
        file.validate_for(identity)?;
        Ok(file)
    }

    /// Reject the whole catalog if a key or identity is ambiguous. An optional sidecar must
    /// not turn a malformed binding into an unrelated actor's animation.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != VERSION {
            return Err("unsupported actor idle catalog version");
        }
        let mut identities = BTreeSet::new();
        for row in &self.bindings {
            for (source, clip) in &row.clips {
                validate_clip(source, clip)?;
            }
            if !matches!(row.sex.as_str(), "male" | "female")
                || !canonical_mesh_key(&row.body_scene, ".glb")
                || !canonical_mesh_key(&row.project, ".hkx")
                || !canonical_mesh_key(&row.skeleton, ".hkx")
                || !canonical_mesh_key(&row.race_skeleton_nif, ".nif")
                || row.bound_tracks == 0
                || row.moving_bound_tracks > row.bound_tracks
                || !canonical_mesh_key(&row.source, ".hkx")
                || !row.scene.starts_with("meshes/actor-idles/clips/")
                || !canonical_mesh_key(&row.scene, ".glb")
                || !identities.insert((row.race_id, &row.sex, &row.body_scene))
            {
                return Err("invalid or duplicate actor idle binding");
            }
        }
        Ok(())
    }

    pub fn find(&self, race_id: u32, sex: &str, body_scene: &str) -> Option<&ActorIdleBinding> {
        self.bindings
            .iter()
            .find(|row| row.race_id == race_id && row.sex == sex && row.body_scene == body_scene)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_keys_reject_encoded_dot_segments() {
        for segment in ["%2e", ".%2e", "%2e.", "%2e%2e"] {
            assert!(!canonical_mesh_key(
                &format!("meshes/{segment}/body.glb"),
                ".glb"
            ));
        }
        assert!(canonical_mesh_key("meshes/horse%20/body.glb", ".glb"));
        assert!(canonical_body_path("meshes/%252e%252e/body.nif", "glb").is_err());
    }

    #[test]
    fn authored_body_keys_decode_once_and_reject_escapes() {
        for path in [
            r"Actors\\Horse\Horse.nif",
            " ./Actors//Horse/./Horse.NIF ",
            "Data/Meshes/Data/Meshes/Actors/Horse/Horse.nif",
            "Actors/%48orse/Horse.nif",
        ] {
            assert_eq!(
                canonical_body_path(path, "glb").unwrap(),
                "meshes/actors/horse/horse.glb",
                "{path}"
            );
        }
        assert_eq!(
            canonical_body_path("Actors/Horse%2520/Body.nif", "glb").unwrap(),
            "meshes/actors/horse%20/body.glb"
        );
        for path in [
            "",
            "meshes/",
            "../body.nif",
            "%2e%2e/body.nif",
            "/body.nif",
            "C:/body.nif",
            "body.nif#Scene0",
            "body.nif?x",
            "body.dds",
            "body%GG.nif",
            "body%00.nif",
        ] {
            assert!(canonical_body_path(path, "glb").is_err(), "{path}");
        }
    }

    #[test]
    fn catalog_rejects_escape_urls_and_duplicate_identities() {
        let binding = ActorIdleBinding {
            race_id: 7,
            sex: "female".into(),
            body_scene: "meshes/body.glb".into(),
            project: "meshes/project.hkx".into(),
            skeleton: "meshes/skeleton.hkx".into(),
            race_skeleton_nif: "meshes/skeleton.nif".into(),
            bound_tracks: 1,
            moving_bound_tracks: 1,
            skipped_tracks: Vec::new(),
            source: "meshes/idle.hkx".into(),
            scene: "meshes/actor-idles/clips/a.glb".into(),
            clips: BTreeMap::new(),
        };
        let mut catalog = ActorIdleCatalog {
            version: VERSION,
            bindings: vec![binding.clone()],
            unresolved: Vec::new(),
            ..Default::default()
        };
        assert!(catalog.validate().is_ok());
        assert!(catalog.find(7, "female", "meshes/body.glb").is_some());
        assert!(catalog.find(8, "female", "meshes/body.glb").is_none());
        catalog.bindings.push(binding);
        assert!(catalog.validate().is_err());
        for key in [
            "meshes/../outside.glb",
            "meshes/a.glb#Scene0",
            "meshes//a.glb",
            r"meshes/a\b.glb",
            "https://example/a.glb",
        ] {
            assert!(!canonical_mesh_key(key, ".glb"));
        }
    }
}
