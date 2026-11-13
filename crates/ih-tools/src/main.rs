//! `ih-tools`: inspect game data through the same file system the engine uses.

mod coverage;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use ih_vfs::{Installs, ModProfile, Vfs};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Mod profile (TOML). Defaults to the stock game.
    #[arg(long, global = true)]
    profile: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show the detected game installs.
    Installs,
    /// List mounted archives in mount order.
    Archives,
    /// List files. The pattern is a folder plus a file mask, e.g. "Data\INI\Object\*.ini".
    Ls {
        #[arg(default_value = "*")]
        pattern: String,
        /// Only the folder itself, not subfolders (archives always recurse, as in the original).
        #[arg(long)]
        shallow: bool,
    },
    /// Show which source provides a file, and what it overrides.
    Which { path: String },
    /// Copy files out of the virtual file system. The pattern works as for `ls`.
    Extract { pattern: String, out: PathBuf },
    /// Print a text file to stdout.
    Cat { path: String },
    /// Print the INI files the game loads at startup, in order, with where each comes from.
    LoadOrder,
    /// Decode textures: save one as PNG, or check every texture the game can load.
    Textures {
        /// Texture name as a model refers to it (e.g. avcrusader.tga). Omit to scan all.
        name: Option<String>,
        /// Write the decoded texture here (PNG).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Render a model to a PNG, offscreen.
    Render {
        /// Model name, e.g. AVCrusader.
        name: String,
        #[arg(long, default_value = "out/render.png")]
        out: PathBuf,
        /// Animation (`SKELETON.ANIM` or file name) and frame to pose the model.
        #[arg(long)]
        anim: Option<String>,
        #[arg(long, default_value_t = 0.0)]
        frame: f32,
        #[arg(long, default_value_t = 512)]
        size: u32,
        /// Camera yaw and pitch in degrees.
        #[arg(long, default_value_t = -135.0)]
        yaw: f32,
        #[arg(long, default_value_t = 35.0)]
        pitch: f32,
        /// House colour as RRGGBB hex (default: USA blue).
        #[arg(long, default_value = "1e5aff")]
        house: String,
        /// Sub-objects to hide (prefix of the mesh name), e.g. TURRETFX.
        #[arg(long, value_delimiter = ',')]
        hide: Vec<String>,
    },
    /// Render several models into one grid image (left to right, top to bottom).
    Sheet {
        /// Models as NAME or NAME:RRGGBB (house colour).
        names: Vec<String>,
        #[arg(long, default_value = "out/sheet.png")]
        out: PathBuf,
        #[arg(long, default_value_t = 384)]
        cell: u32,
        #[arg(long, default_value_t = 3)]
        columns: u32,
        #[arg(long, default_value_t = -110.0, allow_hyphen_values = true)]
        yaw: f32,
        #[arg(long, default_value_t = 35.0)]
        pitch: f32,
        /// Sub-objects to hide (prefix of the mesh name).
        #[arg(long, value_delimiter = ',', default_value = "TURRETFX,MUZZLE")]
        hide: Vec<String>,
    },
    /// Parse W3D models: describe one file, or check every .w3d the game can load.
    Models {
        /// Model file name (e.g. AVCrusader or Art\W3D\AVCrusader.w3d). Omit to scan all.
        name: Option<String>,
    },
    /// Report how much of the mounted data ironhour understands (Markdown).
    Coverage {
        /// Also write the report to this file.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Load the game's string table; look up labels, or print a summary.
    Strings {
        /// Labels to look up (case-insensitive).
        labels: Vec<String>,
        /// A map's folder, e.g. "Maps\Tournament Desert", to add its map.str.
        #[arg(long)]
        map: Option<String>,
    },
    /// Parse and check INI files: by default exactly the files the game loads, in load
    /// order, plus every per-map override file.
    Scan {
        /// Scan every *.ini under these folders instead of following the load order.
        dirs: Vec<String>,
        /// Also print the most used module class names.
        #[arg(long)]
        modules: bool,
        /// Also print how many fields use each C++ value parser.
        #[arg(long)]
        parsers: bool,
    },
}

fn mount(cli: &Cli) -> Result<Vfs> {
    let installs = Installs::locate();
    if installs.zero_hour.is_none() {
        bail!(
            "Zero Hour install not found. Set {} (and {} for base Generals).",
            ih_vfs::install::ENV_ZERO_HOUR,
            ih_vfs::install::ENV_GENERALS
        );
    }
    let profile = match &cli.profile {
        Some(p) => ModProfile::load(p)?,
        None => ModProfile::stock(),
    };
    let vfs = Vfs::for_game(&installs, &profile)?;
    for w in vfs.warnings() {
        eprintln!("warning: {}", w.0);
    }
    Ok(vfs)
}

/// Splits "dir\mask" into ("dir", "mask"). A pattern without wildcards is a folder.
fn split_pattern(pattern: &str) -> (String, String) {
    match pattern.rfind(['\\', '/']) {
        Some(i) if pattern[i + 1..].contains(['*', '?']) => (pattern[..i].to_string(), pattern[i + 1..].to_string()),
        _ if pattern.contains(['*', '?']) => (String::new(), pattern.to_string()),
        _ => (pattern.to_string(), "*".to_string()),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Installs => {
            let i = Installs::locate();
            println!("Zero Hour: {}", i.zero_hour.as_ref().map_or("not found".into(), |p| p.display().to_string()));
            println!("Generals:  {}", i.generals.as_ref().map_or("not found".into(), |p| p.display().to_string()));
        }
        Cmd::Archives => {
            let vfs = mount(&cli)?;
            for (i, m) in vfs.archives().iter().enumerate() {
                println!(
                    "{:3} {} {:6} files  {}",
                    i,
                    if m.overwrite { "mod " } else { "    " },
                    m.archive.entries().len(),
                    m.archive.path().display()
                );
            }
            let loose = vfs.loose_root().map_or(0, |l| l.len());
            println!(
                "{} archives, {} reachable archive files, {} loose files",
                vfs.archives().len(),
                vfs.archive_file_count(),
                loose
            );
        }
        Cmd::Ls { pattern, shallow } => {
            let vfs = mount(&cli)?;
            let (dir, mask) = split_pattern(pattern);
            let files = vfs.list(&dir, &mask, !shallow);
            for f in &files {
                println!("{f}");
            }
            eprintln!("{} files", files.len());
        }
        Cmd::Which { path } => {
            let vfs = mount(&cli)?;
            let sources = vfs.all_sources(path);
            if sources.is_empty() {
                bail!("{path}: not found");
            }
            for (i, s) in sources.iter().enumerate() {
                println!("{} {}", if i == 0 { "used:      " } else { "overridden:" }, s.describe());
            }
        }
        Cmd::Extract { pattern, out } => {
            let vfs = mount(&cli)?;
            let files = if vfs.exists(pattern) {
                vec![pattern.clone()]
            } else {
                let (dir, mask) = split_pattern(pattern);
                vfs.list(&dir, &mask, true)
            };
            if files.is_empty() {
                bail!("{pattern}: no files");
            }
            let mut n = 0;
            for f in &files {
                let Some(bytes) = vfs.read(f)? else { continue };
                let dest = out.join(f.replace('\\', std::path::MAIN_SEPARATOR_STR));
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).with_context(|| parent.display().to_string())?;
                }
                std::fs::write(&dest, bytes).with_context(|| dest.display().to_string())?;
                n += 1;
            }
            eprintln!("extracted {n} files to {}", out.display());
        }
        Cmd::Cat { path } => {
            let vfs = mount(&cli)?;
            let bytes = vfs.read(path)?.with_context(|| format!("{path}: not found"))?;
            print!("{}", String::from_utf8_lossy(&bytes));
        }
        Cmd::Strings { labels, map } => {
            let vfs = mount(&cli)?;
            let (main, from) = load_strings(&vfs, &Installs::locate().language)?;
            let map_table = match map {
                Some(m) => {
                    let p = format!("{}\\map.str", m.trim_end_matches(['\\', '/']));
                    let bytes = vfs.read(&p)?.with_context(|| format!("{p}: not found"))?;
                    Some(ih_formats::strings::parse_str(&bytes))
                }
                None => None,
            };
            if labels.is_empty() {
                let speech = main.entries.iter().filter(|e| e.speech.is_some()).count();
                println!("{from}: {} strings ({speech} with speech), language id {}", main.len(), main.language_id);
                if let Some(t) = &map_table {
                    println!("map.str: {} strings", t.len());
                }
            }
            for l in labels {
                println!("{l} = {:?}", ih_formats::strings::fetch(&main, map_table.as_ref(), l));
            }
        }
        Cmd::Coverage { out } => {
            let vfs = mount(&cli)?;
            let language = Installs::locate().language;
            let (strings, _) = load_strings(&vfs, &language)?;
            let name = match &cli.profile {
                Some(p) => ModProfile::load(p)?.name,
                None => ModProfile::stock().name,
            };
            let text = coverage::run(&vfs, &language, &name, &strings)?;
            print!("{text}");
            if let Some(path) = out {
                std::fs::write(path, &text).with_context(|| path.display().to_string())?;
            }
        }
        Cmd::Models { name } => models(&mount(&cli)?, name.as_deref())?,
        Cmd::Sheet { names, out, cell, columns, yaw, pitch, hide } => {
            let vfs = mount(&cli)?;
            let language = Installs::locate().language;
            let mut assets = ih_render::Assets::new(&vfs, &language);
            let mut renderer = ih_render::Renderer::new()?;
            let cols = (*columns).max(1);
            let rows = (names.len() as u32).div_ceil(cols).max(1);
            let (w, h) = (cell * cols, cell * rows);
            let mut sheet: Vec<u8> = std::iter::repeat_n([46u8, 51, 56, 255], (w * h) as usize).flatten().collect();
            for (i, spec) in names.iter().enumerate() {
                let (name, color) = match spec.split_once(':') {
                    Some((n, c)) => (n, u32::from_str_radix(c, 16).ok()),
                    None => (spec.as_str(), Some(0x1e5aff)),
                };
                let opts = ih_render::BuildOptions { house_color: color, hide: hide.clone() };
                let model = match ih_render::build(&mut assets, name, &opts) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("{name}: {e}");
                        continue;
                    }
                };
                let bones = match &model.hierarchy {
                    Some(hier) => ih_render::pose::base_pose(hier, glam::Mat4::IDENTITY),
                    None => vec![glam::Mat4::IDENTITY],
                };
                let Some((lo, hi)) = ih_render::bounds(&model, &bones) else { continue };
                let camera = ih_render::framing_camera(lo, hi, *yaw, *pitch);
                let rgba = renderer.render(
                    &mut assets,
                    &model,
                    &bones,
                    &camera,
                    &ih_render::Lights::afternoon(),
                    (*cell, *cell),
                    [0.18, 0.2, 0.22, 1.0],
                    0.0,
                );
                let (cx, cy) = (i as u32 % cols * cell, i as u32 / cols * cell);
                for y in 0..*cell {
                    let src = (y * cell * 4) as usize;
                    let dst = (((cy + y) * w + cx) * 4) as usize;
                    sheet[dst..dst + (cell * 4) as usize].copy_from_slice(&rgba[src..src + (cell * 4) as usize]);
                }
                println!("{:2}: {name} ({} batches)", i + 1, model.batches.len());
            }
            write_png(out, w, h, &sheet)?;
            for warn in &assets.warnings {
                eprintln!("warning: {warn}");
            }
            println!("wrote {}", out.display());
        }
        Cmd::Render { name, out, anim, frame, size, yaw, pitch, house, hide } => {
            let vfs = mount(&cli)?;
            let language = Installs::locate().language;
            let mut assets = ih_render::Assets::new(&vfs, &language);
            let opts = ih_render::BuildOptions {
                house_color: Some(u32::from_str_radix(house, 16).context("--house must be RRGGBB hex")?),
                hide: hide.clone(),
            };
            let model = ih_render::build(&mut assets, name, &opts)?;
            let root = glam::Mat4::IDENTITY;
            let bones = match (&model.hierarchy, anim) {
                (Some(h), Some(a)) => {
                    let a = assets.animation(a).with_context(|| format!("animation {a} not found"))?;
                    ih_render::pose::animated_pose(h, &a, *frame, root).0
                }
                (Some(h), None) => ih_render::pose::base_pose(h, root),
                (None, _) => vec![root],
            };
            let (lo, hi) = ih_render::bounds(&model, &bones).context("model has no visible geometry")?;
            let camera = ih_render::framing_camera(lo, hi, *yaw, *pitch);
            let mut renderer = ih_render::Renderer::new()?;
            let rgba = renderer.render(
                &mut assets,
                &model,
                &bones,
                &camera,
                &ih_render::Lights::afternoon(),
                (*size, *size),
                [0.18, 0.2, 0.22, 1.0],
                *frame * 1000.0 / 30.0,
            );
            write_png(out, *size, *size, &rgba)?;
            for w in &assets.warnings {
                eprintln!("warning: {w}");
            }
            println!(
                "{name}: {} batches, {} bones, on {}; wrote {}",
                model.batches.len(),
                bones.len(),
                renderer.adapter_name,
                out.display()
            );
        }
        Cmd::Textures { name, out } => {
            let vfs = mount(&cli)?;
            let language = Installs::locate().language;
            match name {
                Some(n) => {
                    let (path, img) = load_texture(&vfs, &language, n)?.with_context(|| format!("{n}: not found"))?;
                    let translucent = img.rgba().as_chunks::<4>().0.iter().filter(|p| p[3] < 255).count();
                    println!(
                        "{path}: {}x{}, {} mip levels, alpha: {} ({translucent} of {} pixels below 255)",
                        img.width,
                        img.height,
                        img.levels.len(),
                        img.has_alpha,
                        img.width * img.height
                    );
                    if let Some(out) = out {
                        // Colour (alpha forced opaque) and the alpha channel as grey, side by side.
                        let (w, h) = (img.width, img.height);
                        let mut both = vec![0u8; (w * 2 * h * 4) as usize];
                        for y in 0..h {
                            for x in 0..w {
                                let s = ((y * w + x) * 4) as usize;
                                let p = &img.rgba()[s..s + 4];
                                let d = ((y * w * 2 + x) * 4) as usize;
                                both[d..d + 4].copy_from_slice(&[p[0], p[1], p[2], 255]);
                                let d = ((y * w * 2 + w + x) * 4) as usize;
                                both[d..d + 4].copy_from_slice(&[p[3], p[3], p[3], 255]);
                            }
                        }
                        write_png(out, w * 2, h, &both)?;
                        println!("wrote {} (colour | alpha)", out.display());
                    }
                }
                None => {
                    let mut files = vfs.list("Art\\Textures", "*.dds", true);
                    files.extend(vfs.list("Art\\Textures", "*.tga", true));
                    let mut bad = 0;
                    for f in &files {
                        let Some(b) = vfs.read(f)? else { continue };
                        if let Err(e) = ih_formats::texture::decode(&b) {
                            bad += 1;
                            if bad <= 20 {
                                println!("{f}: {e}");
                            }
                        }
                    }
                    println!("{} textures, {bad} failed to decode", files.len());
                }
            }
        }
        Cmd::LoadOrder => {
            use ih_ini::load_order::{BEFORE_MODS, STARTUP, plan};
            let vfs = mount(&cli)?;
            let (files, missing) = plan(&vfs, &Installs::locate().language, STARTUP);
            for (i, p) in files.iter().enumerate() {
                let mods =
                    if STARTUP[..BEFORE_MODS].iter().any(|s| std::ptr::eq(s, p.step)) { " (before -mod)" } else { "" };
                let from = vfs.resolve(&p.path).map(|s| s.describe()).unwrap_or_default();
                println!("{:4} {:<24} {}{mods}\n       {from}", i + 1, p.step.subsystem, p.path);
            }
            for m in &missing {
                println!("MISSING {:?} ({})", m.source, m.cpp);
            }
        }
        Cmd::Scan { dirs, modules, parsers } => {
            scan(&mount(&cli)?, &Installs::locate().language, dirs, *modules, *parsers)?
        }
    }
    Ok(())
}

/// Finds and decodes a texture the way the game does (`W3DFileSystem.cpp`,
/// `textureloader.cpp`): the `.dds` of the same name first, then the `.tga`, each
/// looked up in `Data\<language>\Art\Textures\` and then `Art\Textures\`.
fn load_texture(vfs: &Vfs, language: &str, name: &str) -> Result<Option<(String, ih_formats::texture::Image)>> {
    let base = name.rsplit_once('.').map_or(name, |(b, _)| b);
    for ext in ["dds", "tga"] {
        for dir in [format!("Data\\{language}\\Art\\Textures\\"), "Art\\Textures\\".to_string()] {
            let path = format!("{dir}{base}.{ext}");
            if let Some(bytes) = vfs.read(&path)? {
                return Ok(Some((path, ih_formats::texture::decode(&bytes)?)));
            }
        }
    }
    Ok(None)
}

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path).with_context(|| path.display().to_string())?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

fn models(vfs: &Vfs, name: Option<&str>) -> Result<()> {
    use ih_formats::w3d;
    use std::collections::BTreeMap;

    if let Some(name) = name {
        let path = if name.contains(['\\', '/']) { name.to_string() } else { format!("Art\\W3D\\{name}.w3d") };
        let bytes = vfs.read(&path)?.with_context(|| format!("{path}: not found"))?;
        let f = w3d::parse(&bytes)?;
        for h in &f.hierarchies {
            println!("hierarchy {} ({} pivots)", h.name, h.pivots.len());
        }
        for h in &f.hlods {
            println!(
                "hlod {} (hierarchy {}), {} lods, {} aggregates, {} proxies",
                h.name,
                h.hierarchy,
                h.lods.len(),
                h.aggregates.len(),
                h.proxies.len()
            );
            for (i, l) in h.lods.iter().enumerate() {
                let objs: Vec<_> = l.objects.iter().map(|o| format!("{}@{}", o.name, o.bone)).collect();
                println!("  lod {i} (max size {}): {}", l.max_screen_size, objs.join(", "));
            }
        }
        for m in &f.meshes {
            let mats = m.active_materials();
            let tex: Vec<_> = mats.map(|s| s.textures.iter().map(|t| t.name.clone()).collect()).unwrap_or_default();
            println!(
                "mesh {} : {} verts, {} tris, flags {:#010x}{}, passes {}, textures {:?}",
                m.full_name(),
                m.vertices.len(),
                m.triangles.len(),
                m.attributes,
                if m.is_skin() { " (skin)" } else { "" },
                mats.map_or(0, |s| s.passes.len()),
                tex
            );
        }
        for a in &f.animations {
            println!(
                "animation {} on {}: {} frames at {} fps, {} channels{}",
                a.name,
                a.hierarchy,
                a.num_frames,
                a.frame_rate,
                a.channels.len(),
                if a.compressed { " (compressed)" } else { "" }
            );
        }
        for (id, n) in &f.others {
            println!("other {id:#06x} {n}");
        }
        return Ok(());
    }

    let files = vfs.list("Art\\W3D", "*.w3d", true);
    let (mut meshes, mut hlods, mut hiers, mut anims, mut errors) = (0, 0, 0, 0, 0);
    let mut skipped: BTreeMap<u32, u32> = BTreeMap::new();
    let mut others: BTreeMap<u32, u32> = BTreeMap::new();
    let mut compressed = 0usize;
    let mut features: BTreeMap<&str, usize> = BTreeMap::new();
    let mut mappers: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    let mut mapper_examples: BTreeMap<(u32, u32), String> = BTreeMap::new();
    let mut examples: BTreeMap<&str, String> = BTreeMap::new();
    let mut objects: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut hier_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut wanted: Vec<(String, String)> = Vec::new(); // (object name, used by)
    let mut wanted_h: Vec<(String, String)> = Vec::new();
    for p in &files {
        let Some(bytes) = vfs.read(p)? else { continue };
        match w3d::parse(&bytes) {
            Ok(f) => {
                meshes += f.meshes.len();
                hlods += f.hlods.len();
                hiers += f.hierarchies.len();
                anims += f.animations.len();
                for (k, v) in &f.skipped {
                    *skipped.entry(*k).or_default() += v;
                }
                for (k, _) in &f.others {
                    *others.entry(*k).or_default() += 1;
                }
                for m in &f.meshes {
                    objects.insert(m.full_name().to_ascii_uppercase());
                }
                for b in &f.boxes {
                    objects.insert(b.name.to_ascii_uppercase());
                }
                for (_, n) in &f.others {
                    objects.insert(n.to_ascii_uppercase());
                }
                for m in &f.meshes {
                    let geo = m.attributes & ih_formats::w3d::mesh_flags::GEOMETRY_TYPE_MASK;
                    if geo == ih_formats::w3d::mesh_flags::GEOMETRY_TYPE_CAMERA_ALIGNED
                        || geo == ih_formats::w3d::mesh_flags::GEOMETRY_TYPE_CAMERA_ORIENTED
                    {
                        *features.entry("camera-aligned/oriented meshes").or_default() += 1;
                    }
                    if let Some(set) = m.active_materials() {
                        for v in &set.vertex_materials {
                            for (stage, shift) in [(0u32, 16u32), (1, 8)] {
                                let kind = (v.info.attributes >> shift) & 0xFF;
                                if kind != 0 {
                                    *mappers.entry((stage, kind)).or_default() += 1;
                                    mapper_examples
                                        .entry((stage, kind))
                                        .or_insert_with(|| format!("{} in {p}", m.full_name()));
                                }
                            }
                        }
                        if set.shaders.iter().any(|s| s.sec_gradient != 0) {
                            *features.entry("meshes with specular (secondary gradient)").or_default() += 1;
                        }
                        if set.textures.iter().any(|t| t.info.is_some_and(|i| i.frame_count > 1)) {
                            *features.entry("meshes with animated textures").or_default() += 1;
                        }
                        if set.passes.len() > 1 {
                            *features.entry("multi-pass meshes").or_default() += 1;
                        }
                        if set.passes.iter().any(|p| p.stages.len() > 1) {
                            *features.entry("meshes with a second texture stage").or_default() += 1;
                        }
                    }
                }
                for a in &f.animations {
                    if a.compressed {
                        compressed += 1;
                        let flavor = a.channels.first().map(|c| match c.data {
                            ih_formats::w3d::ChannelData::Dense { .. } => "adaptive-delta",
                            ih_formats::w3d::ChannelData::TimeCoded { .. } => "time-coded",
                            _ => "raw",
                        });
                        if let Some(fl) = flavor {
                            examples.entry(fl).or_insert_with(|| format!("{} ({p})", a.name));
                        }
                    }
                }
                for h in &f.hierarchies {
                    hier_names.insert(h.name.to_ascii_uppercase());
                }
                for h in &f.hlods {
                    objects.insert(h.name.to_ascii_uppercase());
                    for o in h.lods.iter().flat_map(|l| &l.objects).chain(&h.aggregates) {
                        wanted.push((o.name.to_ascii_uppercase(), format!("{p} ({})", h.name)));
                    }
                    if !h.hierarchy.is_empty() {
                        wanted_h.push((h.hierarchy.to_ascii_uppercase(), h.name.clone()));
                    }
                }
                for a in &f.animations {
                    wanted_h.push((a.hierarchy.to_ascii_uppercase(), format!("animation {}", a.name)));
                }
                for e in &f.errors {
                    errors += 1;
                    if errors <= 20 {
                        println!("{p}: {e} (object skipped)");
                    }
                }
            }
            Err(e) => {
                errors += 1;
                if errors <= 20 {
                    println!("{p}: {e}");
                }
            }
        }
    }
    println!(
        "{} files: {meshes} meshes, {hlods} HLODs, {hiers} hierarchies, {anims} animations; {errors} objects or files failed",
        files.len()
    );
    let missing: Vec<_> = wanted.iter().filter(|(n, _)| !objects.contains(n)).collect();
    let missing_h: Vec<_> = wanted_h.iter().filter(|(n, _)| !hier_names.contains(n)).collect();
    println!("HLOD sub-objects: {} references, {} not found in any file", wanted.len(), missing.len());
    for (n, by) in missing.iter().take(10) {
        println!("  {n} used by {by}");
    }
    println!("hierarchy references: {} , {} not found", wanted_h.len(), missing_h.len());
    for (n, by) in missing_h.iter().take(10) {
        println!("  {n} used by {by}");
    }
    println!("features (of {meshes} meshes): {features:?}");
    println!("texture mappers (stage, type) -> vertex materials: {mappers:?}");
    for (k, v) in &mapper_examples {
        println!("  mapper {k:?} e.g. {v}");
    }
    println!("compressed animations: {compressed}, e.g. {examples:?}");
    println!("other render objects: {others:x?}");
    println!("skipped chunk ids: {skipped:x?}");
    Ok(())
}

/// The main string table, as `GameTextManager::init` picks it: a loose
/// `Data\Generals.str` if present, otherwise `Data\<language>\Generals.csf`.
fn load_strings(vfs: &Vfs, language: &str) -> Result<(ih_formats::strings::StringTable, String)> {
    if let Some(b) = vfs.read("Data\\Generals.str")? {
        return Ok((ih_formats::strings::parse_str(&b), "Data\\Generals.str".into()));
    }
    let p = format!("Data\\{language}\\Generals.csf");
    let b = vfs.read(&p)?.with_context(|| format!("{p}: not found"))?;
    Ok((ih_formats::strings::parse_csf(&b)?, p))
}

fn scan(vfs: &Vfs, language: &str, dirs: &[String], show_modules: bool, show_parsers: bool) -> Result<()> {
    use std::collections::BTreeMap;

    let mut files = Vec::new();
    if dirs.is_empty() {
        use ih_ini::load_order::{ON_DEMAND, STARTUP, map_files, plan};
        let (startup, missing) = plan(vfs, language, STARTUP);
        for m in &missing {
            println!("missing required file: {:?} ({})", m.source, m.cpp);
        }
        let (later, _) = plan(vfs, language, ON_DEMAND);
        files.extend(startup.into_iter().chain(later).map(|p| p.path));
        files.extend(map_files(vfs));
        println!("load order: {} files, {} required files missing", files.len(), missing.len());
    } else {
        for d in dirs {
            files.extend(vfs.list(d, "*.ini", true));
        }
    }
    // Some files load twice (ShellMenuScheme); check each once.
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| seen.insert(ih_vfs::path::key(f)));
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut nested: BTreeMap<String, usize> = BTreeMap::new();
    let mut module_classes: BTreeMap<String, usize> = BTreeMap::new();
    let (mut lines, mut fields, mut problems, mut bad_files) = (0u64, 0u64, 0usize, 0usize);
    let mut report = ih_ini::validate::Report::default();
    let (mut values_ok, mut invalid_count) = (0u64, 0u64);
    let mut invalid: Vec<String> = Vec::new();
    let mut unported: BTreeMap<&'static str, u64> = BTreeMap::new();

    for f in &files {
        let Some(bytes) = vfs.read(f)? else { continue };
        let doc = ih_ini::parse(&bytes);
        report.validate_with(f, &doc, &mut |r| match ih_ini::values::parse_field(r.table, r.def, &r.field.rest) {
            Ok(_) => values_ok += 1,
            Err(ih_ini::values::ValueError::Unimplemented(p)) => *unported.entry(p).or_default() += 1,
            Err(ih_ini::values::ValueError::Invalid(msg)) => {
                if invalid.len() < 40 || std::env::var_os("IH_SCAN_ALL").is_some() {
                    invalid.push(format!(
                        "{f}: line {}: {} = {}: {msg}",
                        r.field.line,
                        r.field.name,
                        r.field.rest.trim()
                    ));
                }
                invalid_count += 1;
            }
        });
        lines += u64::from(doc.line_count);
        if !doc.diagnostics.is_empty() {
            bad_files += 1;
            for d in doc.diagnostics.iter().take(5) {
                println!("{f}: {d}");
            }
            if doc.diagnostics.len() > 5 {
                println!("{f}: ... {} more", doc.diagnostics.len() - 5);
            }
            problems += doc.diagnostics.len();
        }
        for b in &doc.blocks {
            *kinds.entry(b.kind.clone()).or_default() += 1;
            b.walk(&mut |blk, depth| {
                fields += blk.fields().count() as u64;
                if depth > 0 {
                    *nested.entry(blk.kind.clone()).or_default() += 1;
                    if matches!(blk.kind.as_str(), "Behavior" | "Body" | "Draw" | "ClientUpdate")
                        && let Some(class) = blk.args().into_iter().next()
                    {
                        *module_classes.entry(class).or_default() += 1;
                    }
                }
            });
        }
    }

    println!("\n{} files, {lines} lines, {fields} fields", files.len());
    println!("{} top-level blocks of {} kinds:", kinds.values().sum::<usize>(), kinds.len());
    let mut v: Vec<_> = kinds.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for chunk in v.chunks(4) {
        println!("  {}", chunk.iter().map(|(k, n)| format!("{k:<28}{n:>6}")).collect::<Vec<_>>().join(""));
    }
    println!("{} nested blocks of {} kinds", nested.values().sum::<usize>(), nested.len());
    println!("{} distinct module classes used", module_classes.len());
    if show_modules {
        let mut m: Vec<_> = module_classes.into_iter().collect();
        m.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (k, n) in m {
            println!("  {k:<40}{n:>6}");
        }
    }
    if problems == 0 {
        println!("structure: OK, no problems");
    } else {
        println!("structure: {problems} problems in {bad_files} files");
    }

    // Schema check.
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for (file, issue) in &report.issues {
        let kind = format!("{issue:?}").split([' ', '{']).next().unwrap_or("").to_string();
        let n = by_kind.entry(kind).or_default();
        *n += 1;
        if *n <= 15 || std::env::var_os("IH_SCAN_ALL").is_some() {
            println!("{file}: {issue}");
        }
    }
    if report.issues.is_empty() {
        println!("schema: OK, all {} fields known", report.fields);
    } else {
        let summary: Vec<_> = by_kind.iter().map(|(k, n)| format!("{n} {k}")).collect();
        println!("schema: {} issues in {} fields ({})", report.issues.len(), report.fields, summary.join(", "));
    }
    for line in &invalid {
        println!("{line}");
    }
    let unported_total: u64 = unported.values().sum();
    let total = values_ok + invalid_count + unported_total;
    println!(
        "values: {values_ok} parsed, {invalid_count} invalid, {unported_total} with parsers not yet ported ({:.1}% of {total} done, {} parsers left)",
        100.0 * values_ok as f64 / total.max(1) as f64,
        unported.len()
    );
    if show_parsers {
        let mut u: Vec<_> = unported.iter().collect();
        u.sort_by(|a, b| b.1.cmp(a.1));
        println!("parsers not yet ported:");
        for (p, n) in u {
            println!("  {n:>8}  {p}");
        }
    }
    if show_parsers {
        let mut v: Vec<_> = report.parser_use.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        println!("{} value parsers used:", v.len());
        for (p, n) in v {
            println!("  {n:>8}  {p}");
        }
    }
    Ok(())
}
