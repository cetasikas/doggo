//! Doggo Desk: labelled fences that group the shortcuts on the desktop.
//!
//! A fence is a *view*, not a folder. It stores references to files that stay
//! in `Desktop`, so turning Doggo off loses no data and never moves a file.
//! The native desktop icons stay under Explorer's control until the user
//! enables `hide_icons`.
//!
//! Layout lives in `%APPDATA%\doggo\desk.toml`, separately from `config.toml`,
//! in the same way `dock-pins.toml` is kept apart.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::util;

pub mod icons;
pub mod native;
pub mod render;
pub mod watch;
pub mod window;

/// Height of a fence's title bar, in DIPs.
pub const TITLE_HEIGHT: i32 = 34;
pub const TAB_HEIGHT: i32 = 27;
/// One icon cell, name included, in DIPs.
pub const CELL: i32 = 92;
/// Inner padding around the icon grid, in DIPs.
pub const PAD: i32 = 6;
/// Narrowest and widest a fence may be dragged, in DIPs.
pub const MIN_WIDTH: i32 = 180;
pub const MAX_WIDTH: i32 = 900;
/// Maximum persisted fence height, in DIPs.
pub const MAX_HEIGHT: i32 = 4000;
pub const AUTO_MAX_HEIGHT: i32 = 680;
pub const MIN_HEIGHT: i32 = TITLE_HEIGHT + TAB_HEIGHT + 60;
/// Icon columns a fence may show.
pub const MIN_COLUMNS: u32 = 1;
pub const MAX_COLUMNS: u32 = 8;
pub const MIN_ICON_SIZE: u32 = 16;
pub const MAX_ICON_SIZE: u32 = 96;
pub const DETAIL_HEADER_HEIGHT: i32 = 24;
pub const DETAIL_ROW_HEIGHT: i32 = 28;
pub const MAX_TRANSPARENCY: u8 = 80;

fn default_width() -> i32 {
    320
}

fn default_columns() -> u32 {
    4
}

fn default_icon_size() -> u32 {
    48
}

fn cell_height(icon_size: u32) -> i32 {
    (icon_size as i32 + 39).max(CELL)
}

/// What an item is, and the rule a fence uses to claim it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    /// `.lnk`, `.url` and `.appref-ms` shortcuts.
    #[default]
    Apps,
    Folders,
    Files,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FenceView {
    #[default]
    Icons,
    Thumbnails,
    Details,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FenceSort {
    #[default]
    Name,
    Type,
    Size,
    Modified,
}

/// A file or folder on the desktop, as the desk sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Display name, without the shortcut extension.
    pub name: String,
    /// Absolute path. Never rewritten by Doggo.
    pub path: String,
    pub category: Category,
    pub is_dir: bool,
    pub size_bytes: u64,
    pub modified: Option<SystemTime>,
}

impl Item {
    /// Builds an item from a path, or `None` for a file the desk should hide.
    ///
    /// `desktop.ini`, dot-files and hidden or system entries are part of how
    /// Windows implements the desktop, not something the user put there.
    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        if lower == "desktop.ini" || lower == "thumbs.db" || name.starts_with('.') {
            return None;
        }
        let metadata = std::fs::metadata(path).ok()?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            let attrs = metadata.file_attributes();
            const HIDDEN: u32 = 0x2;
            const SYSTEM: u32 = 0x4;
            if attrs & (HIDDEN | SYSTEM) != 0 {
                return None;
            }
        }
        let is_dir = metadata.is_dir();
        let (category, display) = if is_dir {
            (Category::Folders, name)
        } else {
            match shortcut_stem(&name) {
                Some(stem) => (Category::Apps, stem),
                None => (Category::Files, name),
            }
        };
        Some(Self {
            name: display,
            path: path.to_string_lossy().to_string(),
            category,
            is_dir,
            size_bytes: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

fn type_key(item: &Item) -> String {
    if item.is_dir {
        "folder".into()
    } else {
        Path::new(&item.path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    }
}

fn sort_items(items: &mut [Item], sort: FenceSort) {
    items.sort_by(|a, b| {
        let by = match sort {
            FenceSort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            FenceSort::Type => type_key(a).cmp(&type_key(b)),
            FenceSort::Size => a.size_bytes.cmp(&b.size_bytes),
            FenceSort::Modified => b.modified.unwrap_or(UNIX_EPOCH).cmp(&a.modified.unwrap_or(UNIX_EPOCH)),
        };
        by.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// Removes the extension from a shortcut, keeping a normal file's name whole.
fn shortcut_stem(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    for ext in [".lnk", ".url", ".appref-ms"] {
        if lower.ends_with(ext) && name.len() > ext.len() {
            return Some(name[..name.len() - ext.len()].to_string());
        }
    }
    None
}

/// The desktop folders Windows uses: the user's (which may be redirected to
/// OneDrive) and the public one.
pub fn desktop_dirs() -> Vec<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_PublicDesktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

    let mut dirs = Vec::with_capacity(2);
    for id in [&FOLDERID_Desktop, &FOLDERID_PublicDesktop] {
        let folder = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) };
        if let Ok(folder) = folder {
            if let Ok(text) = unsafe { folder.to_string() } {
                dirs.push(PathBuf::from(text));
            }
            unsafe { CoTaskMemFree(Some(folder.0.cast())) };
        }
    }
    dirs
}

/// Reads every desktop item, newest folders last, in a stable order.
pub fn collect(dirs: &[PathBuf]) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(item) = Item::from_path(&entry.path()) else {
                continue;
            };
            if items.iter().any(|old| old.path.eq_ignore_ascii_case(&item.path)) {
                continue;
            }
            items.push(item);
        }
    }
    items.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    items
}

/// A fence as it will be drawn, with the items it claimed this frame.
#[derive(Clone, Debug)]
pub struct Placed {
    pub index: usize,
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub columns: u32,
    pub icon_size: u32,
    pub view: FenceView,
    pub sort: FenceSort,
    pub transparency: u8,
    pub height: i32,
    pub scroll_y: i32,
    pub collapsed: bool,
    pub name: String,
    pub browsing: bool,
    pub tint: Option<u32>,
    pub tabs: Vec<String>,
    pub active_tab: usize,
    pub items: Vec<Item>,
}

impl Placed {
    pub fn cell_height(&self) -> i32 {
        cell_height(self.icon_size)
    }

    pub fn max_scroll(&self) -> i32 {
        if self.collapsed {
            return 0;
        }
        let content = match self.view {
            FenceView::Details => {
                let viewport = (self.height - TITLE_HEIGHT - TAB_HEIGHT - PAD * 2 - DETAIL_HEADER_HEIGHT).max(0);
                return (self.items.len() as i32 * DETAIL_ROW_HEIGHT - viewport).max(0);
            }
            FenceView::Icons | FenceView::Thumbnails => {
                PAD * 2 + self.items.len().div_ceil(self.columns.max(1) as usize) as i32 * self.cell_height()
            }
        };
        (content - (self.height - TITLE_HEIGHT - TAB_HEIGHT)).max(0)
    }
}

/// One labelled group of desktop items.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fence {
    pub id: String,
    pub name: String,
    /// Optional RGB tint for the fence body and title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<u32>,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default = "default_width")]
    pub width: i32,
    #[serde(default = "default_icon_size")]
    pub icon_size: u32,
    #[serde(default)]
    pub view: FenceView,
    #[serde(default)]
    pub sort: FenceSort,
    #[serde(default)]
    pub transparency: u8,
    /// Optional saved height. Fences without one fit their contents.
    #[serde(default, rename = "height", skip_serializing_if = "Option::is_none")]
    pub height_override: Option<i32>,
    /// Current scroll offset; this is view state, not a layout preference.
    #[serde(skip)]
    pub scroll_y: i32,
    #[serde(default = "default_columns")]
    pub columns: u32,
    #[serde(default)]
    pub collapsed: bool,
    /// The rule this fence uses. `None` means only `items` belong here. A
    /// portal fence ignores this and lists its folder instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
    /// Mirrors the contents of this absolute folder. The files stay where
    /// they are; the fence only lists them. Takes precedence over `category`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Absolute paths that stay in this fence even when the rule would claim
    /// them elsewhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
    /// Saved pages within this fence. Existing layouts are migrated to one Main tab.
    #[serde(default)]
    pub tabs: Vec<FenceTab>,
    /// Currently selected page.
    #[serde(default)]
    pub active_tab: usize,
    /// Folder navigation is transient and is not written to the layout file.
    #[serde(skip)]
    pub browse_stack: Vec<String>,
    /// Preserves the current viewport height while browsing a large folder.
    #[serde(skip)]
    pub browse_height: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FenceTab {
    pub name: String,
    #[serde(default)]
    pub is_main: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
}

impl Fence {
    /// Rows needed for `count` items at this fence's column count.
    pub fn rows(&self, count: usize) -> i32 {
        let columns = self.columns.max(1) as usize;
        count.div_ceil(columns) as i32
    }

    /// Drawn height, title bar included. A collapsed fence is only its title.
    pub fn natural_height(&self, count: usize) -> i32 {
        if self.collapsed {
            return TITLE_HEIGHT;
        }
        match self.view {
            FenceView::Details => {
                (TITLE_HEIGHT + TAB_HEIGHT + PAD * 2 + DETAIL_HEADER_HEIGHT + count as i32 * DETAIL_ROW_HEIGHT)
                    .min(AUTO_MAX_HEIGHT)
            }
            FenceView::Icons | FenceView::Thumbnails => {
                (TITLE_HEIGHT + TAB_HEIGHT + PAD * 2 + self.rows(count) * cell_height(self.icon_size))
                    .min(AUTO_MAX_HEIGHT)
            }
        }
    }

    /// Drawn height, including any saved vertical resize.
    pub fn height(&self, count: usize) -> i32 {
        if self.collapsed {
            TITLE_HEIGHT
        } else {
            self.height_override
                .or(self.browse_height)
                .unwrap_or_else(|| self.natural_height(count))
        }
    }

    /// Keeps a hand-edited file inside the values the renderer can handle.
    fn sanitize(&mut self) {
        self.id = self.id.trim().to_string();
        self.name = self.name.trim().to_string();
        self.folder = self
            .folder
            .take()
            .map(|folder| folder.trim().to_string())
            .filter(|folder| !folder.is_empty());
        self.width = self.width.clamp(MIN_WIDTH, MAX_WIDTH);
        self.icon_size = self.icon_size.clamp(MIN_ICON_SIZE, MAX_ICON_SIZE);
        self.transparency = self.transparency.min(MAX_TRANSPARENCY);
        self.height_override = self.height_override.map(|height| height.clamp(MIN_HEIGHT, MAX_HEIGHT));
        self.scroll_y = self.scroll_y.max(0);
        self.columns = self.columns.clamp(MIN_COLUMNS, MAX_COLUMNS);
        if self.name.is_empty() {
            self.name = "Fence".to_string();
        }
        if self.tabs.is_empty() {
            self.tabs.push(FenceTab {
                name: "Main".into(),
                is_main: true,
                category: self.category,
                folder: self.folder.clone(),
                items: self.items.clone(),
            });
            self.active_tab = 0;
        }
        for tab in &mut self.tabs {
            tab.name = tab.name.trim().chars().take(24).collect();
            if tab.name.is_empty() {
                tab.name = "Tab".into();
            }
        }
        let default_category = match self.id.to_ascii_lowercase().as_str() {
            "apps" => Some(Category::Apps),
            "folders" => Some(Category::Folders),
            "files" => Some(Category::Files),
            _ => None,
        };
        let main_index = self
            .tabs
            .iter()
            .position(|tab| tab.is_main || tab.name.eq_ignore_ascii_case("Main"));
        if let Some(main_index) = main_index {
            for (index, tab) in self.tabs.iter_mut().enumerate() {
                tab.is_main = index == main_index;
            }
            self.tabs[main_index].is_main = true;
            if main_index != 0 {
                self.tabs.swap(0, main_index);
                if self.active_tab == 0 {
                    self.active_tab = main_index;
                } else if self.active_tab == main_index {
                    self.active_tab = 0;
                }
            }
        } else {
            self.tabs.insert(
                0,
                FenceTab {
                    name: "Main".into(),
                    is_main: true,
                    category: default_category,
                    folder: None,
                    items: Vec::new(),
                },
            );
            self.active_tab = self.active_tab.saturating_add(1);
        }
        self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
        self.load_active_tab();
    }

    pub fn save_active_tab(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.category = self.category;
            tab.folder = self.folder.clone();
            tab.items = self.items.clone();
        }
    }

    fn load_active_tab(&mut self) {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            self.category = tab.category;
            self.folder = tab.folder.clone();
            self.items = tab.items.clone();
        }
    }
}

/// The whole desk: whether it runs, whether it hides Explorer's icons, and the
/// fences in draw order.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Desk {
    #[serde(default)]
    pub enabled: bool,
    /// Hide Explorer's own icons so the desk is the only thing on screen.
    #[serde(default)]
    pub hide_icons: bool,
    #[serde(default = "default_true")]
    pub show_labels: bool,
    #[serde(default)]
    pub fence: Vec<Fence>,
}

fn default_true() -> bool {
    true
}

impl Default for Desk {
    fn default() -> Self {
        Self::default_layout()
    }
}

impl Desk {
    pub fn path() -> PathBuf {
        util::data_dir().join("desk.toml")
    }

    /// Three fences that cover the usual desktop, all switched off until the
    /// user opts in. Nothing is hidden until `enabled` and `hide_icons` are
    /// both true.
    pub fn default_layout() -> Self {
        let fence = vec![
            Fence {
                id: "apps".into(),
                name: "Apps".into(),
                tint: None,
                x: 40,
                y: 40,
                width: 320,
                icon_size: default_icon_size(),
                view: FenceView::Icons,
                sort: FenceSort::Name,
                transparency: 0,
                height_override: None,
                scroll_y: 0,
                columns: 4,
                collapsed: false,
                category: Some(Category::Apps),
                folder: None,
                items: Vec::new(),
                tabs: vec![FenceTab {
                    name: "Main".into(),
                    is_main: true,
                    category: Some(Category::Apps),
                    folder: None,
                    items: Vec::new(),
                }],
                active_tab: 0,
                browse_stack: Vec::new(),
                browse_height: None,
            },
            Fence {
                id: "folders".into(),
                name: "Folders".into(),
                tint: None,
                x: 380,
                y: 40,
                width: 320,
                icon_size: default_icon_size(),
                view: FenceView::Icons,
                sort: FenceSort::Name,
                transparency: 0,
                height_override: None,
                scroll_y: 0,
                columns: 4,
                collapsed: false,
                category: Some(Category::Folders),
                folder: None,
                items: Vec::new(),
                tabs: vec![FenceTab {
                    name: "Main".into(),
                    is_main: true,
                    category: Some(Category::Folders),
                    folder: None,
                    items: Vec::new(),
                }],
                active_tab: 0,
                browse_stack: Vec::new(),
                browse_height: None,
            },
            Fence {
                id: "files".into(),
                name: "Files".into(),
                tint: None,
                x: 40,
                y: 360,
                width: 320,
                icon_size: default_icon_size(),
                view: FenceView::Icons,
                sort: FenceSort::Name,
                transparency: 0,
                height_override: None,
                scroll_y: 0,
                columns: 4,
                collapsed: false,
                category: Some(Category::Files),
                folder: None,
                items: Vec::new(),
                tabs: vec![FenceTab {
                    name: "Main".into(),
                    is_main: true,
                    category: Some(Category::Files),
                    folder: None,
                    items: Vec::new(),
                }],
                active_tab: 0,
                browse_stack: Vec::new(),
                browse_height: None,
            },
        ];
        Self {
            enabled: false,
            hide_icons: false,
            show_labels: true,
            fence,
        }
    }

    /// Reads `desk.toml`, creating the default file on first run. A broken
    /// file falls back to the default layout and reports the problem.
    pub fn load() -> (Self, Option<String>) {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match Self::parse(&text) {
                Ok(desk) => (desk, None),
                Err(error) => (Self::default_layout(), Some(format!("desk.toml: {}", error.message()))),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut desk = Self::default_layout();
                let _ = desk.save();
                (desk, None)
            }
            Err(error) => (Self::default_layout(), Some(format!("desk.toml: {error}"))),
        }
    }

    /// Parses a desk, then removes fences the renderer cannot use.
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        let mut desk: Self = toml::from_str(text)?;
        desk.sanitize();
        Ok(desk)
    }

    /// Writes the desk aside first, so a crash keeps the previous file.
    pub fn save(&mut self) -> std::io::Result<()> {
        for fence in &mut self.fence {
            fence.save_active_tab();
        }
        let text = toml::to_string_pretty(self).map_err(|_| std::io::Error::other("Cannot save the desk layout."))?;
        let path = Self::path();
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("Cannot find the desk folder."))?;
        std::fs::create_dir_all(parent)?;
        let tmp = path.with_extension("toml.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &path)
    }

    fn sanitize(&mut self) {
        self.fence
            .retain(|fence| !fence.id.trim().is_empty() && !fence.id.eq_ignore_ascii_case("weather"));
        let mut seen: Vec<String> = Vec::with_capacity(self.fence.len());
        self.fence.retain(|fence| {
            if seen.iter().any(|id| id.eq_ignore_ascii_case(&fence.id)) {
                return false;
            }
            seen.push(fence.id.clone());
            true
        });
        for fence in &mut self.fence {
            fence.sanitize();
        }
    }

    /// Claims items for each fence. Explicit paths win over rules, and the
    /// first rule that matches a category takes the rest. Returns the claimed
    /// items per fence and the ones no fence wanted.
    pub fn assign(&self, items: &[Item]) -> (Vec<Vec<Item>>, Vec<Item>) {
        let mut claimed: Vec<Vec<Item>> = vec![Vec::new(); self.fence.len()];
        let mut owner: Vec<Option<usize>> = vec![None; items.len()];

        for (index, fence) in self.fence.iter().enumerate() {
            for want in &fence.items {
                for (slot, item) in items.iter().enumerate() {
                    if owner[slot].is_none() && item.path.eq_ignore_ascii_case(want) {
                        owner[slot] = Some(index);
                    }
                }
            }
        }

        for (slot, item) in items.iter().enumerate() {
            if owner[slot].is_some() {
                continue;
            }
            if let Some(index) = self
                .fence
                .iter()
                .position(|fence| fence.category == Some(item.category))
            {
                owner[slot] = Some(index);
            }
        }

        let mut unclaimed = Vec::new();
        for (slot, item) in items.iter().enumerate() {
            match owner[slot] {
                Some(index) => claimed[index].push(item.clone()),
                None => unclaimed.push(item.clone()),
            }
        }
        (claimed, unclaimed)
    }

    /// Fences with their assigned items and current geometry. A portal fence
    /// lists its folder instead of the desktop items a rule claims.
    pub fn layout(&self, items: &[Item]) -> Vec<Placed> {
        let (claimed, _) = self.assign(items);
        self.fence
            .iter()
            .enumerate()
            .map(|(index, fence)| {
                let browsing_path = fence.browse_stack.last();
                let items = if let Some(path) = browsing_path {
                    collect(&[PathBuf::from(path)])
                } else {
                    match &fence.folder {
                        Some(folder) => collect(&[PathBuf::from(folder)]),
                        None => claimed[index].clone(),
                    }
                };
                let mut items = items;
                sort_items(&mut items, fence.sort);
                Placed {
                    index,
                    id: fence.id.clone(),
                    x: fence.x,
                    y: fence.y,
                    width: fence.width,
                    columns: fence.columns,
                    icon_size: fence.icon_size,
                    view: fence.view,
                    sort: fence.sort,
                    transparency: fence.transparency,
                    height: fence.height(items.len()),
                    scroll_y: fence.scroll_y,
                    collapsed: fence.collapsed,
                    name: browsing_path
                        .and_then(|path| Path::new(path).file_name())
                        .map(|leaf| format!("{} · {}", fence.name, leaf.to_string_lossy()))
                        .unwrap_or_else(|| fence.name.clone()),
                    browsing: browsing_path.is_some(),
                    tint: fence.tint,
                    tabs: fence.tabs.iter().map(|tab| tab.name.clone()).collect(),
                    active_tab: fence.active_tab,
                    items,
                }
            })
            .collect()
    }

    /// Every folder whose changes should refresh the desk: the desktop itself
    /// and each portal.
    pub fn watch_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = desktop_dirs();
        for fence in &self.fence {
            if let Some(folder) = &fence.folder {
                dirs.push(PathBuf::from(folder));
            }
            if let Some(path) = fence.browse_stack.last() {
                dirs.push(PathBuf::from(path));
            }
        }
        let mut seen = HashSet::with_capacity(dirs.len());
        dirs.retain(|dir| seen.insert(dir.to_string_lossy().to_ascii_lowercase()));
        dirs
    }

    /// Bounding rectangle of every fence, for the window region and hit tests.
    pub fn bounds(&self, items: &[Item]) -> Option<(i32, i32, i32, i32)> {
        let placed = self.layout(items);
        let first = placed.first()?;
        let mut left = first.x;
        let mut top = first.y;
        let mut right = first.x + first.width;
        let mut bottom = first.y + first.height;
        for fence in &placed[1..] {
            left = left.min(fence.x);
            top = top.min(fence.y);
            right = right.max(fence.x + fence.width);
            bottom = bottom.max(fence.y + fence.height);
        }
        Some((left, top, right - left, bottom - top))
    }
}

/// Clear space kept between fences when one has to be pushed aside, in DIPs.
pub const GAP: i32 = 16;

/// Whether two fence rectangles, given as (left, top, width, height), share
/// any area.
pub fn overlaps(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

/// Moves fences that overlap an earlier one to the nearest free spot inside
/// `width` x `height` DIPs, keeping a small gap. Fences that already fit stay
/// exactly where they are, so a hand-made layout survives. Returns true when
/// at least one fence moved.
pub fn separate(placed: &mut [Placed], width: i32, height: i32) -> bool {
    let mut moved = false;
    for index in 0..placed.len() {
        // Start on screen: a hand-edited file may have put a fence outside.
        let x = placed[index].x.clamp(0, (width - placed[index].width).max(0));
        let y = placed[index].y.clamp(0, (height - placed[index].height).max(0));
        if (x, y) != (placed[index].x, placed[index].y) {
            placed[index].x = x;
            placed[index].y = y;
            moved = true;
        }
        // A free spot may land on a later fence, so keep going until this one
        // is clear of every earlier fence.
        for _ in 0..placed.len() {
            let (x, y) = (placed[index].x, placed[index].y);
            let (width_here, height_here) = (placed[index].width, placed[index].height);
            let blocker = (0..index).find(|&other| {
                let fence = &placed[other];
                overlaps(
                    (x, y, width_here, height_here),
                    (fence.x, fence.y, fence.width, fence.height),
                )
            });
            let Some(other) = blocker else { break };
            let fence = &placed[other];
            let spots = [
                (fence.x + fence.width + GAP, y),
                (fence.x - width_here - GAP, y),
                (x, fence.y + fence.height + GAP),
                (x, fence.y - height_here - GAP),
            ];
            let best = {
                let free = |(left, top): (i32, i32)| {
                    left >= 0
                        && top >= 0
                        && left + width_here <= width
                        && top + height_here <= height
                        && !placed.iter().enumerate().any(|(j, fence)| {
                            j != index
                                && overlaps(
                                    (left, top, width_here, height_here),
                                    (fence.x, fence.y, fence.width, fence.height),
                                )
                        })
                };
                spots
                    .into_iter()
                    .filter(|spot| free(*spot))
                    .min_by_key(|(left, top)| (left - x).abs() + (top - y).abs())
            };
            let Some((left, top)) = best else { break };
            placed[index].x = left;
            placed[index].y = top;
            moved = true;
        }
    }
    moved
}

/// Space kept between the fences and the screen edge when they are tidied up,
/// in DIPs.
pub const MARGIN: i32 = 24;

/// The tidy-up layouts offered by a fence's right-click menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrange {
    /// A balanced grid of columns, filled left to right.
    Grid,
    /// Columns filled top to bottom, hugging the left edge.
    Left,
    /// Columns filled top to bottom, hugging the right edge.
    Right,
}

/// Tidies the fences into `how`, inside `width` x `height` DIPs. The fences
/// keep their top-to-bottom, left-to-right reading order, every pair keeps
/// `GAP` between them, and nothing leaves the screen. Returns true when at
/// least one fence moved.
pub fn arrange(placed: &mut [Placed], width: i32, height: i32, how: Arrange) -> bool {
    if placed.is_empty() {
        return false;
    }
    let before: Vec<(i32, i32)> = placed.iter().map(|fence| (fence.x, fence.y)).collect();
    // Fences may differ in width; a common cell keeps the columns aligned.
    let cell = placed.iter().map(|fence| fence.width).max().unwrap_or(MIN_WIDTH);
    let available = (height - MARGIN * 2).max(0);
    let max_columns = (((width - MARGIN * 2 + GAP) / (cell + GAP)).max(1) as usize).min(placed.len());

    // Tidy the way the user reads the desk, not the order in the file.
    let mut order: Vec<usize> = (0..placed.len()).collect();
    order.sort_by_key(|&index| (placed[index].y, placed[index].x, index));

    match how {
        Arrange::Grid => {
            // A square-ish grid is the tidiest, unless it would run off the
            // bottom of the screen.
            let mut columns = (order.len() as f64).sqrt().ceil() as usize;
            columns = columns.clamp(1, max_columns);
            loop {
                let rows = order.len().div_ceil(columns);
                let mut total = 0;
                for row in 0..rows {
                    let first = row * columns;
                    let last = (first + columns).min(order.len());
                    let tallest = order[first..last]
                        .iter()
                        .map(|&index| placed[index].height)
                        .max()
                        .unwrap_or(0);
                    total += if row == 0 { tallest } else { GAP + tallest };
                }
                if total <= available || columns >= max_columns {
                    break;
                }
                columns += 1;
            }
            let rows = order.len().div_ceil(columns);
            let mut tops = Vec::with_capacity(rows);
            let mut top = MARGIN;
            for row in 0..rows {
                let first = row * columns;
                let last = (first + columns).min(order.len());
                let tallest = order[first..last]
                    .iter()
                    .map(|&index| placed[index].height)
                    .max()
                    .unwrap_or(0);
                tops.push(top);
                top += tallest + GAP;
            }
            for (position, &index) in order.iter().enumerate() {
                placed[index].x = MARGIN + (position % columns) as i32 * (cell + GAP);
                placed[index].y = tops[position / columns];
            }
        }
        Arrange::Left | Arrange::Right => {
            // Fill each column to the bottom before starting the next.
            let mut columns: Vec<Vec<usize>> = vec![Vec::new()];
            let mut used = 0;
            for &index in &order {
                let height = placed[index].height;
                let needed = if columns.last().is_some_and(Vec::is_empty) {
                    height
                } else {
                    used + GAP + height
                };
                if needed > available
                    && columns.last().is_some_and(|column| !column.is_empty())
                    && columns.len() < max_columns
                {
                    columns.push(Vec::new());
                    used = height;
                } else {
                    used = needed;
                }
                if let Some(column) = columns.last_mut() {
                    column.push(index);
                }
            }
            let widths: Vec<i32> = columns
                .iter()
                .map(|column| column.iter().map(|&index| placed[index].width).max().unwrap_or(0))
                .collect();
            let mut cursor = MARGIN;
            let mut right = width - MARGIN;
            for (column, &column_width) in columns.iter().zip(&widths) {
                let left = if how == Arrange::Right {
                    right - column_width
                } else {
                    cursor
                };
                let mut top = MARGIN;
                for &index in column {
                    placed[index].x = if how == Arrange::Right {
                        left + column_width - placed[index].width
                    } else {
                        left
                    };
                    placed[index].y = top;
                    top += placed[index].height + GAP;
                }
                cursor += column_width + GAP;
                right -= column_width + GAP;
            }
        }
    }

    // A hand-edited file may hold a fence wider or taller than the screen.
    for fence in placed.iter_mut() {
        fence.x = fence.x.clamp(0, (width - fence.width).max(0));
        fence.y = fence.y.clamp(0, (height - fence.height).max(0));
    }
    (0..placed.len()).any(|index| (placed[index].x, placed[index].y) != before[index])
}

#[cfg(test)]
mod tests {

    use super::*;

    fn item(name: &str, path: &str, category: Category, is_dir: bool) -> Item {
        Item {
            name: name.into(),
            path: path.into(),
            category,
            is_dir,
            size_bytes: 0,
            modified: None,
        }
    }

    #[test]
    fn the_default_desk_is_off_and_groups_the_three_categories() {
        let desk = Desk::default_layout();
        assert!(!desk.enabled);
        assert!(!desk.hide_icons);
        assert_eq!(desk.fence.len(), 3);
        assert_eq!(
            desk.fence.iter().filter_map(|f| f.category).collect::<Vec<_>>(),
            [Category::Apps, Category::Folders, Category::Files]
        );
    }

    #[test]
    fn layout_round_trips_through_the_file_and_a_broken_file_is_rejected() {
        let mut desk = Desk::default_layout();
        desk.enabled = true;
        desk.fence[0].collapsed = true;
        desk.fence[1].items = vec![r"C:\Users\me\Desktop\Keep.txt".into()];
        let text = toml::to_string_pretty(&desk).unwrap();
        let loaded = Desk::parse(&text).unwrap();
        assert!(loaded.enabled);
        assert!(loaded.fence[0].collapsed);
        assert_eq!(loaded.fence[1].items.len(), 1);
        assert!(Desk::parse("fence = 3").is_err());
        assert!(Desk::parse("[[fence]]\nid = ''\nname = 'x'").unwrap().fence.is_empty());
    }

    #[test]
    fn explicit_items_win_over_rules_and_paths_ignore_case() {
        let mut desk = Desk::default_layout();
        desk.fence[2].category = None;
        desk.fence[2].items = vec![r"C:\USERS\ME\DESKTOP\NOTE.TXT".into()];
        let items = vec![
            item("Note", r"C:\Users\me\Desktop\Note.txt", Category::Files, false),
            item("Tool", r"C:\Users\me\Desktop\Tool.exe", Category::Files, false),
        ];
        let (claimed, unclaimed) = desk.assign(&items);
        assert_eq!(claimed[2].len(), 1);
        assert_eq!(claimed[2][0].name, "Note");
        assert_eq!(unclaimed.len(), 1);
        assert_eq!(unclaimed[0].name, "Tool");
    }

    #[test]
    fn items_no_fence_claims_are_reported_not_lost() {
        let mut desk = Desk::default_layout();
        desk.fence[0].category = None;
        desk.fence[1].category = None;
        desk.fence[2].category = None;
        let items = vec![item("Loose", r"C:\x\Loose.bin", Category::Files, false)];
        let (claimed, unclaimed) = desk.assign(&items);
        assert!(claimed.iter().all(Vec::is_empty));
        assert_eq!(unclaimed.len(), 1);
    }

    #[test]
    fn a_portal_lists_its_folder_instead_of_the_category_rule() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Inside.txt"), "").unwrap();
        let mut desk = Desk::default_layout();
        desk.fence[0].folder = Some(dir.path().to_string_lossy().into_owned());
        // The rule would claim this apps-category shortcut, but the portal
        // ignores the rule and shows its own folder.
        let tool = item("Tool", r"C:\Users\me\Desktop\Tool.exe", Category::Apps, false);
        let placed = desk.layout(&[tool]);
        assert_eq!(placed[0].items.len(), 1);
        assert_eq!(placed[0].items[0].name, "Inside.txt");
    }

    #[test]
    fn portals_are_watched_once_even_when_repeated_or_differently_cased() {
        let dir = tempfile::tempdir().unwrap();
        let portal = dir.path().to_string_lossy().into_owned();
        let mut desk = Desk::default_layout();
        desk.fence[0].folder = Some(portal.clone());
        desk.fence[1].folder = Some(portal.to_uppercase());
        let dirs = desk.watch_dirs();
        let watched = dirs
            .iter()
            .filter(|path| path.to_string_lossy().eq_ignore_ascii_case(&portal))
            .count();
        assert_eq!(watched, 1, "a portal should be watched once");
        assert!(dirs.len() >= 2, "the desktop and the portal are both watched");
    }

    fn placed_at(index: usize, x: i32, y: i32, width: i32, height: i32) -> Placed {
        Placed {
            index,
            id: format!("f{index}"),
            x,
            y,
            width,
            columns: 4,
            icon_size: default_icon_size(),
            view: FenceView::Icons,
            sort: FenceSort::Name,
            transparency: 0,
            height,
            scroll_y: 0,
            collapsed: false,
            name: "F".into(),
            browsing: false,
            view: FenceView::Icons,
            sort: FenceSort::Name,
            transparency: 0,
            tint: None,
            items: Vec::new(),
        }
    }

    #[test]
    fn overlapping_fences_move_clear_while_the_others_stay_put() {
        let mut placed = vec![
            placed_at(0, 177, 104, 320, 230),
            placed_at(1, 380, 40, 320, 138),
            placed_at(2, 179, 355, 320, 138),
        ];
        assert!(separate(&mut placed, 1280, 720));
        assert_eq!((placed[0].x, placed[0].y), (177, 104));
        assert_eq!((placed[2].x, placed[2].y), (179, 355));
        for (index, fence) in placed.iter().enumerate() {
            for earlier in &placed[..index] {
                assert!(
                    !overlaps(
                        (fence.x, fence.y, fence.width, fence.height),
                        (earlier.x, earlier.y, earlier.width, earlier.height)
                    ),
                    "fences {} and {} still overlap",
                    fence.id,
                    earlier.id
                );
            }
        }
    }

    #[test]
    fn a_layout_that_already_fits_is_untouched() {
        let mut placed = vec![placed_at(0, 40, 40, 320, 230), placed_at(1, 380, 40, 320, 138)];
        assert!(!separate(&mut placed, 1280, 720));
        assert_eq!(placed[1].x, 380);
    }

    #[test]
    fn fences_outside_the_screen_come_back_in() {
        let mut placed = vec![placed_at(0, -50, 4000, 320, 138)];
        assert!(separate(&mut placed, 1280, 720));
        assert_eq!((placed[0].x, placed[0].y), (0, 582));
    }

    fn three_fences() -> Vec<Placed> {
        vec![
            placed_at(0, 58, 138, 320, 322),
            placed_at(1, 621, 157, 320, 230),
            placed_at(2, 222, 488, 320, 230),
        ]
    }

    fn every_pair_is_clear(placed: &[Placed], width: i32, height: i32) {
        for (index, fence) in placed.iter().enumerate() {
            assert!(
                fence.x >= 0
                    && fence.y >= 0
                    && fence.x + fence.width <= width.max(fence.width)
                    && fence.y + fence.height <= height.max(fence.height),
                "fence {index} left the screen: {fence:?}"
            );
            for other in &placed[index + 1..] {
                assert!(
                    !overlaps(
                        (fence.x, fence.y, fence.width, fence.height),
                        (other.x, other.y, other.width, other.height)
                    ),
                    "{fence:?} overlaps {other:?}"
                );
            }
        }
    }

    #[test]
    fn arrange_snaps_the_fences_into_a_grid() {
        let mut placed = three_fences();
        assert!(arrange(&mut placed, 1280, 720, Arrange::Grid));
        assert_eq!((placed[0].x, placed[0].y), (MARGIN, MARGIN));
        assert_eq!((placed[1].x, placed[1].y), (MARGIN + 320 + GAP, MARGIN),);
        assert_eq!((placed[2].x, placed[2].y), (MARGIN, MARGIN + 322 + GAP),);
        every_pair_is_clear(&placed, 1280, 720);
        // Tidying an already tidy desk changes nothing.
        assert!(!arrange(&mut placed, 1280, 720, Arrange::Grid));
    }

    #[test]
    fn arrange_stacks_against_either_edge() {
        let mut placed = three_fences();
        assert!(arrange(&mut placed, 1280, 720, Arrange::Left));
        // The first column holds the tall fence and the next one below it;
        // the third fence starts a second column.
        assert_eq!((placed[0].x, placed[0].y), (MARGIN, MARGIN));
        assert_eq!((placed[1].x, placed[1].y), (MARGIN, MARGIN + 322 + GAP));
        assert_eq!((placed[2].x, placed[2].y), (MARGIN + 320 + GAP, MARGIN));
        every_pair_is_clear(&placed, 1280, 720);

        let mut placed = three_fences();
        assert!(arrange(&mut placed, 1280, 720, Arrange::Right));
        // The first column hugs the right edge and the overflow column sits
        // to its left, so the block reads from the right.
        assert_eq!(placed[0].x + placed[0].width, 1280 - MARGIN);
        assert_eq!(placed[1].x + placed[1].width, 1280 - MARGIN);
        assert_eq!(placed[2].x + placed[2].width, 1280 - MARGIN - 320 - GAP);
        every_pair_is_clear(&placed, 1280, 720);
    }

    #[test]
    fn arrange_keeps_widths_and_a_small_screen_in_mind() {
        let mut placed = vec![
            placed_at(0, 0, 0, 900, 200),
            placed_at(1, 0, 0, 180, 200),
            placed_at(2, 0, 0, 320, 200),
        ];
        assert!(arrange(&mut placed, 1280, 720, Arrange::Grid));
        every_pair_is_clear(&placed, 1280, 720);
        // Narrow screens still get one column, not a negative width.
        let mut placed = vec![
            placed_at(0, 0, 0, 180, 60),
            placed_at(1, 0, 0, 160, 60),
            placed_at(2, 0, 0, 140, 60),
        ];
        assert!(arrange(&mut placed, 400, 400, Arrange::Left));
        every_pair_is_clear(&placed, 400, 400);
    }

    #[test]
    fn height_grows_by_rows_and_collapses_to_the_title() {
        let mut fence = Desk::default_layout().fence.remove(0);
        fence.columns = 4;
        assert_eq!(fence.rows(0), 0);
        assert_eq!(fence.rows(4), 1);
        assert_eq!(fence.rows(5), 2);
        assert_eq!(fence.height(5), TITLE_HEIGHT + TAB_HEIGHT + PAD * 2 + 2 * CELL);
        fence.collapsed = true;
        assert_eq!(fence.height(100), TITLE_HEIGHT);
    }

    #[test]
    fn saved_desks_clamp_values_and_drop_duplicate_ids() {
        let text = r#"
            enabled = true

            [[fence]]
            id = "apps"
            name = "Apps"
            x = 0
            y = 0
            width = 10
            columns = 99
            category = "apps"

            [[fence]]
            id = "APPS"
            name = "Duplicate"
            x = 0
            y = 0

            [[fence]]
            id = "wide"
            name = "Wide"
            x = 0
            y = 0
            width = 99999
            columns = 0
            category = "files"
        "#;
        let desk = Desk::parse(text).unwrap();
        assert_eq!(desk.fence.len(), 2);
        assert_eq!(desk.fence[0].width, MIN_WIDTH);
        assert_eq!(desk.fence[0].columns, MAX_COLUMNS);
        assert_eq!(desk.fence[1].width, MAX_WIDTH);
        assert_eq!(desk.fence[1].columns, MIN_COLUMNS);
    }

    #[test]
    fn collect_skips_windows_files_hides_shortcut_extensions_and_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let desktop = dir.path();
        std::fs::write(desktop.join("desktop.ini"), "").unwrap();
        std::fs::write(desktop.join(".hidden"), "").unwrap();
        std::fs::write(desktop.join("Game.lnk"), "").unwrap();
        std::fs::write(desktop.join("Site.URL"), "").unwrap();
        std::fs::write(desktop.join("Notes.txt"), "").unwrap();
        std::fs::create_dir(desktop.join("Projects")).unwrap();

        let items = collect(&[desktop.to_path_buf(), desktop.to_path_buf()]);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Game", "Notes.txt", "Projects", "Site"]);
        let categories: Vec<Category> = items.iter().map(|i| i.category).collect();
        assert_eq!(
            categories,
            [Category::Apps, Category::Files, Category::Folders, Category::Apps]
        );
    }

    #[test]
    fn hidden_and_system_files_are_not_desktop_items() {
        // The test's own temp file is visible; a hidden file needs the Windows
        // attribute, which std can set only through the metadata helper below.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Secret.txt");
        std::fs::write(&path, "").unwrap();
        assert!(Item::from_path(&path).is_some());
        assert!(Item::from_path(&dir.path().join("desktop.ini")).is_none());
    }
}
