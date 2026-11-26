use crate::schema::{
    BackgroundSource, LayerKind, LayerNode, PackageKind, PackagedMediaItem, PrismManifest,
    PrismScene, StaticOrFx, MANIFEST_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ZIP archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Missing scene.json or theme.json inside package")]
    MissingScene,
    #[error("Unsafe path inside archive: {0}")]
    UnsafePath(String),
}

#[derive(Debug, Clone)]
pub struct ImportedPackage {
    pub scene: PrismScene,
    pub manifest: PrismManifest,
    pub extracted_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteamWorkshopMeta {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub file: String,
    #[serde(rename = "type", default)]
    pub item_type: String,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub description: String,
}

impl SteamWorkshopMeta {
    pub fn from_project_json(path: &Path) -> Result<Self, PackageError> {
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }
}

pub fn sanitize_archive_name(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut prev_underscore = false;
    for ch in title.trim().chars() {
        let valid = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_';
        if valid {
            out.push(ch);
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "PrismView_Scene".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn recommended_extension(kind: PackageKind) -> &'static str {
    match kind {
        PackageKind::Wallpaper => "prism",
        PackageKind::Widget => "widget.prism",
    }
}

pub fn export_package(
    scene: &PrismScene,
    output_path: &Path,
    author: &str,
    font_resolver: impl Fn(&str) -> Option<PathBuf>,
) -> Result<PrismManifest, PackageError> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(output_path)?;
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let mut font_map = BTreeMap::new();
    let mut written_entries = HashSet::new();
    for (node, _) in scene.flatten_layers() {
        if node.kind == LayerKind::Text && !font_map.contains_key(&node.text.font_id) {
            if let Some(font_path) = font_resolver(&node.text.font_id) {
                if font_path.is_file() {
                    if let Some(fname) = font_path.file_name().and_then(|s| s.to_str()) {
                        let entry = format!("fonts/{}", sanitize_file_name(fname));
                        if written_entries.insert(entry.clone()) {
                            write_file_into_zip(&mut zip, &entry, &font_path, opts)?;
                        }
                        font_map.insert(node.text.font_id.clone(), entry);
                    }
                }
            }
        }
    }

    let mut media = Vec::new();
    let mut has_folder = false;
    let mut bg_files: Vec<PathBuf> = Vec::new();
    match &scene.background {
        BackgroundSource::Image { path } | BackgroundSource::Video { path, .. } => {
            let p = PathBuf::from(path);
            if p.is_file() {
                bg_files.push(p);
            }
        }
        BackgroundSource::CarouselFolder { path, .. } => {
            has_folder = true;
            let dir = PathBuf::from(path);
            if dir.is_dir() {
                if let Ok(entries) = fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_file() && is_media_path(&p) {
                            bg_files.push(p);
                        }
                    }
                }
            }
        }
        BackgroundSource::None | BackgroundSource::SteamWorkshop { .. } => {}
    }
    bg_files.sort();

    for (idx, bg_path) in bg_files.iter().enumerate() {
        let orig_name = bg_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("media.png");
        let entry = format!("assets/media_{}_{}", idx, sanitize_file_name(orig_name));
        if written_entries.insert(entry.clone()) {
            write_file_into_zip(&mut zip, &entry, bg_path, opts)?;
        }
        let depth_enabled = scene
            .depth
            .depth_overrides
            .get(orig_name)
            .copied()
            .unwrap_or(scene.depth.enabled);
        media.push(PackagedMediaItem {
            entry_path: entry,
            original_key: orig_name.to_string(),
            depth_enabled,
        });
    }

    let mut custom_asset_map = BTreeMap::new();
    let mut portable_custom_assets = BTreeMap::new();
    for (idx, (alias, spec)) in scene.custom_assets.iter().enumerate() {
        if spec.starts_with("svg:") {
            portable_custom_assets.insert(alias.clone(), spec.clone());
        } else {
            let p = PathBuf::from(spec);
            if p.is_file() {
                let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("png");
                let entry = format!(
                    "custom_assets/asset_{}_{}.{}",
                    idx,
                    sanitize_archive_name(alias),
                    ext
                );
                if written_entries.insert(entry.clone()) {
                    write_file_into_zip(&mut zip, &entry, &p, opts)?;
                }
                custom_asset_map.insert(alias.clone(), entry.clone());
                portable_custom_assets.insert(alias.clone(), entry);
            }
        }
    }

    let mut layer_image_map = BTreeMap::new();
    for (node, _) in scene.flatten_layers() {
        if node.kind == LayerKind::Image {
            let candidate = node.asset.fallback();
            let is_alias = scene.custom_assets.contains_key(candidate);
            if !is_alias && !candidate.is_empty() && !layer_image_map.contains_key(&node.id) {
                let p = PathBuf::from(candidate);
                if p.is_file() {
                    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("png");
                    let entry = format!("layer_assets/layer_{}.{}", sanitize_archive_name(&node.id), ext);
                    if written_entries.insert(entry.clone()) {
                        write_file_into_zip(&mut zip, &entry, &p, opts)?;
                    }
                    layer_image_map.insert(node.id.clone(), entry);
                }
            }
        }
    }

    let exposed_globals = scene
        .globals
        .iter()
        .filter(|g| g.exposed)
        .cloned()
        .collect();

    let manifest = PrismManifest {
        version: MANIFEST_VERSION,
        package_type: scene.package_type,
        is_widget: scene.is_widget(),
        widget_aspect: scene.widget.aspect_ratio,
        title: scene.title.clone(),
        author: author.to_string(),
        preview_path: None,
        has_folder,
        media,
        font_map,
        custom_asset_map,
        layer_image_map,
        exposed_globals,
    };

    let mut portable_scene = scene.clone();
    portable_scene.custom_assets = portable_custom_assets;
    if !matches!(portable_scene.background, BackgroundSource::SteamWorkshop { .. }) {
        portable_scene.background = BackgroundSource::None;
    }

    zip.start_file("manifest.json", opts)?;
    zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;

    let scene_json = serde_json::to_string_pretty(&portable_scene)?;
    zip.start_file("scene.json", opts)?;
    zip.write_all(scene_json.as_bytes())?;

    if scene.is_widget() {
        zip.start_file("widget.json", opts)?;
        zip.write_all(scene_json.as_bytes())?;
    }

    zip.finish()?;
    Ok(manifest)
}

pub fn inspect_manifest(archive_path: &Path) -> Result<PrismManifest, PackageError> {
    let file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(file)?;
    for name in ["manifest.json", "bundle.json"] {
        if let Ok(mut entry) = archive.by_name(name) {
            let mut raw = String::new();
            entry.read_to_string(&mut raw)?;
            if let Ok(m) = serde_json::from_str::<PrismManifest>(&raw) {
                return Ok(m);
            }
        }
    }
    Ok(PrismManifest {
        is_widget: archive_path
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains(".widget."),
        ..PrismManifest::default()
    })
}

pub fn import_package(
    archive_path: &Path,
    storage_root: &Path,
) -> Result<ImportedPackage, PackageError> {
    let file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(file)?;
    let new_id = Uuid::new_v4().to_string();
    let extract_dir = storage_root.join(&new_id);
    let assets_dir = extract_dir.join("assets");
    let custom_dir = extract_dir.join("custom_assets");
    let layer_dir = extract_dir.join("layer_assets");
    let fonts_dir = extract_dir.join("fonts");
    fs::create_dir_all(&assets_dir)?;
    fs::create_dir_all(&custom_dir)?;
    fs::create_dir_all(&layer_dir)?;
    fs::create_dir_all(&fonts_dir)?;

    let mut scene_json: Option<String> = None;
    let mut manifest: Option<PrismManifest> = None;
    let mut saw_widget_json = archive_path
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains(".widget.");

    let mut extracted_fonts = HashMap::<String, PathBuf>::new();
    let mut extracted_assets = HashMap::<String, PathBuf>::new();
    let mut extracted_custom = HashMap::<String, PathBuf>::new();
    let mut extracted_layer = HashMap::<String, PathBuf>::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let rel = entry.name().replace('\\', "/");
        let rel = rel.trim_start_matches('/').to_string();
        if rel.contains("..") {
            return Err(PackageError::UnsafePath(rel));
        }
        let file_name = Path::new(&rel)
            .file_name()
            .and_then(|s| s.to_str())
            .map(sanitize_file_name)
            .unwrap_or_else(|| format!("entry_{i}"));

        if rel == "widget.json" || rel.ends_with("/widget.json") {
            saw_widget_json = true;
            let mut raw = String::new();
            entry.read_to_string(&mut raw)?;
            scene_json = Some(raw);
        } else if (rel == "scene.json" || rel.ends_with("/scene.json") || rel == "theme.json" || rel.ends_with("/theme.json"))
            && scene_json.is_none()
        {
            let mut raw = String::new();
            entry.read_to_string(&mut raw)?;
            scene_json = Some(raw);
        } else if rel == "manifest.json" || rel.ends_with("/manifest.json") || rel == "bundle.json" || rel.ends_with("/bundle.json") {
            let mut raw = String::new();
            entry.read_to_string(&mut raw)?;
            if let Ok(m) = serde_json::from_str::<PrismManifest>(&raw) {
                manifest = Some(m);
            }
        } else if rel.starts_with("fonts/") || rel.ends_with(".ttf") || rel.ends_with(".otf") {
            let dst = fonts_dir.join(&file_name);
            copy_zip_entry(&mut entry, &dst)?;
            extracted_fonts.insert(rel, dst.clone());
            extracted_fonts.insert(file_name, dst);
        } else if rel.starts_with("custom_assets/") {
            let dst = custom_dir.join(&file_name);
            copy_zip_entry(&mut entry, &dst)?;
            extracted_custom.insert(rel, dst.clone());
            extracted_custom.insert(file_name, dst);
        } else if rel.starts_with("layer_assets/") {
            let dst = layer_dir.join(&file_name);
            copy_zip_entry(&mut entry, &dst)?;
            extracted_layer.insert(rel, dst.clone());
            extracted_layer.insert(file_name, dst);
        } else if rel.starts_with("assets/") || is_media_path(Path::new(&file_name)) {
            let dst = assets_dir.join(&file_name);
            copy_zip_entry(&mut entry, &dst)?;
            extracted_assets.insert(rel, dst.clone());
            extracted_assets.insert(file_name, dst);
        }
    }

    let raw_scene = scene_json.ok_or(PackageError::MissingScene)?;
    let mut scene: PrismScene = serde_json::from_str(&raw_scene)?;
    let manifest = manifest.unwrap_or_default();

    let is_widget = saw_widget_json
        || manifest.is_widget
        || manifest.package_type == PackageKind::Widget
        || scene.is_widget();
    scene.id = new_id;
    scene.package_type = if is_widget {
        PackageKind::Widget
    } else {
        PackageKind::Wallpaper
    };
    if is_widget {
        scene.widget.aspect_ratio = manifest.widget_aspect.clamp(0.6, 4.5);
    }

    let mut restored_custom = BTreeMap::new();
    for (alias, spec) in &scene.custom_assets {
        if spec.starts_with("svg:") {
            restored_custom.insert(alias.clone(), spec.clone());
        } else {
            let rel_key = manifest.custom_asset_map.get(alias).unwrap_or(spec);
            if let Some(dst) = extracted_custom
                .get(rel_key)
                .or_else(|| Path::new(rel_key).file_name().and_then(|n| n.to_str()).and_then(|n| extracted_custom.get(n)))
            {
                restored_custom.insert(alias.clone(), dst.to_string_lossy().to_string());
            }
        }
    }
    scene.custom_assets = restored_custom;

    scene.layers = scene
        .layers
        .iter()
        .map(|l| remap_layer_paths(l, &manifest, &extracted_fonts, &extracted_layer))
        .collect();

    let mut unique_assets: Vec<PathBuf> = extracted_assets
        .values()
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    unique_assets.sort();

    if unique_assets.len() > 1 || (manifest.has_folder && !unique_assets.is_empty()) {
        scene.background = BackgroundSource::CarouselFolder {
            path: assets_dir.to_string_lossy().to_string(),
            interval_min: 0,
            change_on_wake: true,
        };
    } else if let Some(first) = unique_assets.first() {
        let p_str = first.to_string_lossy().to_string();
        scene.background = if is_video_path(first) {
            BackgroundSource::Video {
                path: p_str,
                muted: true,
                loop_video: true,
                playback: Default::default(),
            }
        } else {
            BackgroundSource::Image { path: p_str }
        };
    }

    Ok(ImportedPackage {
        scene,
        manifest,
        extracted_dir: extract_dir,
    })
}

fn remap_layer_paths(
    node: &LayerNode,
    manifest: &PrismManifest,
    extracted_fonts: &HashMap<String, PathBuf>,
    extracted_layer: &HashMap<String, PathBuf>,
) -> LayerNode {
    let mut out = node.clone();
    if let Some(font_entry) = manifest.font_map.get(&node.text.font_id) {
        if let Some(dst) = extracted_fonts.get(font_entry) {
            out.text.font_id = format!("file:{}", dst.to_string_lossy());
        }
    }
    if let Some(layer_entry) = manifest.layer_image_map.get(&node.id) {
        if let Some(dst) = extracted_layer.get(layer_entry) {
            let p_str = dst.to_string_lossy().to_string();
            out.asset = match &node.asset {
                StaticOrFx::Static(_) => StaticOrFx::Static(p_str),
                StaticOrFx::Fx { expr, .. } => StaticOrFx::fx(p_str, expr.clone()),
            };
        }
    }
    out.id = Uuid::new_v4().to_string()[..8].to_string();
    out.children = node
        .children
        .iter()
        .map(|c| remap_layer_paths(c, manifest, extracted_fonts, extracted_layer))
        .collect();
    out
}

fn write_file_into_zip<W: Write + std::io::Seek>(
    zip: &mut ZipWriter<W>,
    entry_path: &str,
    src_path: &Path,
    opts: SimpleFileOptions,
) -> Result<(), PackageError> {
    zip.start_file(entry_path, opts)?;
    let mut f = File::open(src_path)?;
    std::io::copy(&mut f, zip)?;
    Ok(())
}

fn copy_zip_entry<R: Read>(entry: &mut zip::read::ZipFile<'_, R>, dst: &Path) -> Result<(), PackageError> {
    let mut out = File::create(dst)?;
    std::io::copy(entry, &mut out)?;
    Ok(())
}

fn sanitize_file_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn is_video_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "mp4" | "webm" | "mkv" | "mov" | "m4v")
}

fn is_media_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "heic" | "mp4" | "webm" | "mkv" | "mov" | "m4v"
    )
}
