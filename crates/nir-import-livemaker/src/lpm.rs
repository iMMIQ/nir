//! Clean parser for LiveMaker preview menus: both binary LPM106 (`LivePrevMenu106`)
//! and XML LPM200 (`<PrevMenu><Version>200</Version>`).
//!
//! Resolves relative button/background/preview image paths (including `..\`) against
//! the `.lpm` file's parent directory inside the VFS.

use crate::lsb;
use anyhow::{bail, ensure, Context, Result};
use nir_import_core::vfs::normalize_vfs_path;

#[derive(Debug, Clone)]
pub struct LpmButton {
    pub name: String,
    pub left: i32,
    pub top: i32,
    pub image: String,
    pub hover_image: Option<String>,
    pub disabled_image: Option<String>,
    pub preview_image: Option<String>,
    pub preview_pos: [i32; 2],
}

#[derive(Debug, Clone)]
pub struct LpmMenu {
    pub width: u32,
    pub height: u32,
    pub background: Option<String>,
    pub buttons: Vec<LpmButton>,
}

/// Parses an `.lpm` file (binary LPM106 or XML LPM200) and resolves relative paths
/// against `lpm_vfs_path`'s directory.
pub fn parse_lpm(data: &[u8], lpm_vfs_path: &str) -> Result<LpmMenu> {
    let base_dir = lpm_vfs_path
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or("");
    let mut menu = if data.starts_with(b"LivePrevMenu106") {
        parse_lpm106(data)?
    } else {
        parse_lpm200(data)?
    };
    if let Some(bg) = menu.background.take() {
        menu.background = resolve_rel(base_dir, &bg);
    }
    for btn in &mut menu.buttons {
        btn.image = resolve_rel(base_dir, &btn.image).unwrap_or_default();
        btn.hover_image = btn
            .hover_image
            .take()
            .and_then(|p| resolve_rel(base_dir, &p));
        btn.disabled_image = btn
            .disabled_image
            .take()
            .and_then(|p| resolve_rel(base_dir, &p));
        btn.preview_image = btn
            .preview_image
            .take()
            .and_then(|p| resolve_rel(base_dir, &p));
    }
    Ok(menu)
}

fn resolve_rel(base_dir: &str, raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let clean = trimmed.replace('\\', "/");
    let joined = if base_dir.is_empty()
        || clean.starts_with("グラフィック/")
        || clean.starts_with("ノベルシステム/")
        || clean.starts_with("サウンド/")
    {
        clean
    } else {
        format!("{base_dir}/{clean}")
    };
    Some(normalize_vfs_path(&joined))
}

struct BinReader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> BinReader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .context("E_IMPORT_LPM: offset overflow")?;
        let slice = self
            .data
            .get(self.at..end)
            .context("E_IMPORT_LPM: truncated LPM106")?;
        self.at = end;
        Ok(slice)
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn string(&mut self) -> Result<String> {
        let n = self.i32()?;
        ensure!(
            (0..=1024 * 1024).contains(&n),
            "E_IMPORT_LPM: invalid string length"
        );
        lsb::decode(self.take(n as usize)?)
    }
}

fn parse_lpm106(data: &[u8]) -> Result<LpmMenu> {
    let mut r = BinReader { data, at: 0 };
    ensure!(
        r.take(15)? == b"LivePrevMenu106",
        "E_IMPORT_LPM: invalid LPM106 magic"
    );
    let width = r.i32()?.clamp(1, 8192) as u32;
    let height = r.i32()?.clamp(1, 8192) as u32;
    let count = r.i32()?;
    ensure!(
        (0..=512).contains(&count),
        "E_IMPORT_LPM: invalid button count"
    );
    let mut buttons = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let left = r.i32()?;
        let top = r.i32()?;
        let image = r.string()?;
        let _cap_mask = r.take(1)?[0];
        let name = r.string()?;
        let in_image = r.string()?;
        let _out_image = r.string()?;
        let down_image = r.string()?;
        let _s3 = r.string()?;
        let _s4 = r.string()?;
        let _s5 = r.string()?;
        let _s6 = r.string()?;
        let _s7 = r.string()?;
        let _s8 = r.string()?;
        let _s9 = r.string()?;
        let _s10 = r.string()?;
        let _prev_coords = r.take(8)?;
        let in_prev = r.string()?;
        for _ in 0..5 {
            let _ = r.string()?;
        }
        let _flags = r.take(4)?;
        let _tail1 = r.string()?;
        let _tail2 = r.string()?;

        let hover_image = if !in_image.is_empty() {
            Some(in_image)
        } else if !down_image.is_empty() {
            Some(down_image)
        } else {
            None
        };
        let preview_image = (!in_prev.is_empty()).then_some(in_prev);
        buttons.push(LpmButton {
            name,
            left,
            top,
            image,
            hover_image,
            disabled_image: None,
            preview_image,
            preview_pos: [0, 0],
        });
    }
    ensure!(r.at == data.len(), "E_IMPORT_LPM: trailing LPM106 bytes");
    Ok(LpmMenu {
        width,
        height,
        background: None,
        buttons,
    })
}

fn parse_lpm200(data: &[u8]) -> Result<LpmMenu> {
    ensure!(
        data.len() <= 2 * 1024 * 1024,
        "E_IMPORT_LPM: XML exceeds 2 MiB"
    );
    let text = match std::str::from_utf8(data) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            let (cow, _, _) = encoding_rs::SHIFT_JIS.decode(data);
            cow.into_owned()
        }
    };
    let doc = roxmltree::Document::parse_with_options(
        &text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 32_768,
        },
    )
    .context("E_IMPORT_LPM: invalid XML")?;
    let root = doc.root_element();
    ensure!(
        root.tag_name().name() == "PrevMenu",
        "E_IMPORT_LPM: expected <PrevMenu> root"
    );

    let child_text = |node: roxmltree::Node<'_, '_>, tag: &str| -> Option<String> {
        node.children()
            .find(|n| n.is_element() && n.tag_name().name() == tag)
            .map(|n| n.text().unwrap_or("").trim().to_owned())
    };
    let child_i32 = |node: roxmltree::Node<'_, '_>, tag: &str, default: i32| -> i32 {
        child_text(node, tag)
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(default)
    };

    let version = child_text(root, "Version").unwrap_or_default();
    if !version.is_empty() && version != "200" {
        bail!("E_IMPORT_LPM: unsupported XML PrevMenu version `{version}`");
    }

    let width = child_i32(root, "Width", 1024).clamp(1, 8192) as u32;
    let height = child_i32(root, "Height", 768).clamp(1, 8192) as u32;
    let background = child_text(root, "BGFile").filter(|s| !s.is_empty());

    let mut buttons = Vec::new();
    if let Some(btn_container) = root
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "Button")
    {
        for item in btn_container
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "Item")
        {
            let left = child_i32(item, "Left", 0);
            let top = child_i32(item, "Top", 0);
            let prev_left = child_i32(item, "PrevLeft", 0);
            let prev_top = child_i32(item, "PrevTop", 0);
            let image = child_text(item, "Path").unwrap_or_default();
            let name = child_text(item, "Name")
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("Button{:02}", buttons.len()));
            let hover_image = child_text(item, "InImagePath")
                .filter(|s| !s.is_empty())
                .or_else(|| child_text(item, "SettleImagePath").filter(|s| !s.is_empty()))
                .or_else(|| child_text(item, "DownImagePath").filter(|s| !s.is_empty()));
            let disabled_image = child_text(item, "DisImagePath").filter(|s| !s.is_empty());
            let preview_image = child_text(item, "InPrevPath")
                .filter(|s| !s.is_empty())
                .or_else(|| child_text(item, "DisInPrevPath").filter(|s| !s.is_empty()));

            buttons.push(LpmButton {
                name,
                left,
                top,
                image,
                hover_image,
                disabled_image,
                preview_image,
                preview_pos: [prev_left, prev_top],
            });
        }
    }

    Ok(LpmMenu {
        width,
        height,
        background,
        buttons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lpm200_xml_and_resolves_parent_paths() {
        let xml = r#"<?xml version="1.0" encoding="Shift_JIS"?>
<PrevMenu>
  <Version>200</Version>
  <Width>1280</Width>
  <Height>720</Height>
  <BGFile>..\bg.gal</BGFile>
  <Button>
    <Item>
      <Left>100</Left>
      <Top>200</Top>
      <PrevLeft>400</PrevLeft>
      <PrevTop>50</PrevTop>
      <Path>btn_a.gal</Path>
      <Name>Start</Name>
      <InImagePath>btn_a_hover.gal</InImagePath>
      <DisImagePath>btn_a_dis.gal</DisImagePath>
      <InPrevPath>preview_a.gal</InPrevPath>
    </Item>
  </Button>
</PrevMenu>"#;
        let menu = parse_lpm(xml.as_bytes(), "gfx/ui/title.lpm").unwrap();
        assert_eq!((menu.width, menu.height), (1280, 720));
        assert_eq!(menu.background.as_deref(), Some("gfx/bg.gal"));
        assert_eq!(menu.buttons.len(), 1);
        let b = &menu.buttons[0];
        assert_eq!(b.name, "Start");
        assert_eq!((b.left, b.top), (100, 200));
        assert_eq!(b.image, "gfx/ui/btn_a.gal");
        assert_eq!(b.hover_image.as_deref(), Some("gfx/ui/btn_a_hover.gal"));
        assert_eq!(b.disabled_image.as_deref(), Some("gfx/ui/btn_a_dis.gal"));
        assert_eq!(b.preview_image.as_deref(), Some("gfx/ui/preview_a.gal"));
        assert_eq!(b.preview_pos, [400, 50]);
    }

    #[test]
    fn parses_lpm106_binary() {
        let mut data = b"LivePrevMenu106".to_vec();
        data.extend_from_slice(&1024i32.to_le_bytes());
        data.extend_from_slice(&768i32.to_le_bytes());
        data.extend_from_slice(&1i32.to_le_bytes());
        data.extend_from_slice(&320i32.to_le_bytes());
        data.extend_from_slice(&240i32.to_le_bytes());
        let push_str = |buf: &mut Vec<u8>, s: &str| {
            buf.extend_from_slice(&(s.len() as i32).to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
        };
        push_str(&mut data, "a1.gal");
        data.push(0); // capmask
        push_str(&mut data, "Btn0");
        push_str(&mut data, "a2.gal"); // in_image
        for _ in 0..10 {
            push_str(&mut data, "");
        }
        data.extend_from_slice(&[0u8; 8]);
        push_str(&mut data, ""); // in_prev
        for _ in 0..5 {
            push_str(&mut data, "");
        }
        data.extend_from_slice(&[0u8; 4]);
        push_str(&mut data, "");
        push_str(&mut data, "");

        let menu = parse_lpm(&data, "menu/main.lpm").unwrap();
        assert_eq!((menu.width, menu.height), (1024, 768));
        assert_eq!(menu.buttons.len(), 1);
        assert_eq!(menu.buttons[0].name, "Btn0");
        assert_eq!(menu.buttons[0].image, "menu/a1.gal");
        assert_eq!(menu.buttons[0].hover_image.as_deref(), Some("menu/a2.gal"));
    }
}
