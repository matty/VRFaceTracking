//! Reads NumPy `.npz` files: a zip of `.npy` arrays, here little-endian
//! float32 or float64, or a unicode string.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context, Result};

#[derive(Clone, Debug)]
pub enum Array {
    Float { shape: Vec<usize>, values: Vec<f32> },
    Text(String),
}

impl Array {
    pub fn floats(&self, name: &str) -> Result<(&[usize], &[f32])> {
        match self {
            Array::Float { shape, values } => Ok((shape, values)),
            Array::Text(_) => bail!("{name} is text, not numbers"),
        }
    }
}

fn field<'h>(header: &'h str, key: &str) -> Result<&'h str> {
    let start = header
        .find(&format!("'{key}':"))
        .with_context(|| format!("npy header has no {key}"))?
        + key.len()
        + 3;
    Ok(header[start..].trim_start())
}

/// One `.npy` file's array.
pub fn parse_npy(bytes: &[u8]) -> Result<Array> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        bail!("not an npy array");
    }
    let (header_len, start) = match bytes[6] {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 => (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        ),
        version => bail!("npy version {version}"),
    };
    let header = std::str::from_utf8(&bytes[start..start + header_len]).context("npy header")?;
    let data = &bytes[start + header_len..];
    let descr = field(header, "descr")?;
    let descr = descr
        .trim_start_matches('\'')
        .split('\'')
        .next()
        .unwrap_or_default();
    if field(header, "fortran_order")?.starts_with("True") {
        bail!("Fortran-ordered npy arrays aren't read");
    }
    let shape_text = field(header, "shape")?;
    let shape_text = &shape_text[1..shape_text.find(')').context("npy shape")?];
    let shape: Vec<usize> = shape_text
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse())
        .collect::<Result<_, _>>()
        .context("npy shape")?;
    let count: usize = shape.iter().product();
    let array = match descr {
        "<f4" => Array::Float {
            values: data[..count * 4]
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
            shape,
        },
        "<f8" => Array::Float {
            values: data[..count * 8]
                .chunks_exact(8)
                .map(|b| f64::from_le_bytes(b.try_into().unwrap()) as f32)
                .collect(),
            shape,
        },
        unicode if unicode.starts_with("<U") => {
            let chars: usize = unicode[2..].parse().context("npy string width")?;
            let text: String = data[..chars * 4 * count.max(1)]
                .chunks_exact(4)
                .filter_map(|b| char::from_u32(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
                .take_while(|&c| c != '\0')
                .collect();
            Array::Text(text)
        }
        other => bail!("npy type {other} isn't read"),
    };
    Ok(array)
}

/// Every array in an `.npz` file, by name.
pub fn read(path: &Path) -> Result<HashMap<String, Array>> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("an npz file is a zip")?;
    let mut arrays = HashMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().trim_end_matches(".npy").to_string();
        let mut bytes = vec![];
        entry.read_to_end(&mut bytes)?;
        arrays.insert(
            name.clone(),
            parse_npy(&bytes).with_context(|| format!("{} {name}", path.display()))?,
        );
    }
    Ok(arrays)
}
