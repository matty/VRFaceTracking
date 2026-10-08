//! Reads and writes NumPy `.npz` files: a zip of `.npy` arrays, here
//! little-endian float32 or float64, or a unicode string.

use std::collections::HashMap;
use std::io::{Read, Write};
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

/// One array as a `.npy` file, version 1.0, as NumPy writes it.
pub fn npy(array: &Array) -> Vec<u8> {
    let (descr, shape, data) = match array {
        Array::Float { shape, values } => (
            "<f4".to_string(),
            shape.clone(),
            values
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>(),
        ),
        Array::Text(text) => (
            format!("<U{}", text.chars().count().max(1)),
            vec![],
            text.chars()
                .map(u32::from)
                .chain(text.is_empty().then_some(0))
                .flat_map(u32::to_le_bytes)
                .collect(),
        ),
    };
    let shape = match shape.as_slice() {
        [] => "()".to_string(),
        [only] => format!("({only},)"),
        dims => format!(
            "({})",
            dims.iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let mut header = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
    // The data starts on a 64-byte boundary, after a newline.
    let padding = (64 - (10 + header.len() + 1) % 64) % 64;
    header.push_str(&" ".repeat(padding));
    header.push('\n');
    let mut out = b"\x93NUMPY\x01\x00".to_vec();
    out.extend_from_slice(&(header.len() as u16).to_le_bytes());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(&data);
    out
}

/// Writes `arrays`, in order, as an uncompressed `.npz`, as `np.savez`
/// does, through a temporary file.
pub fn write(path: &Path, arrays: &[(String, Array)]) -> Result<()> {
    let temporary = path.with_extension("npz.tmp");
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&temporary)?);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    for (name, array) in arrays {
        archive.start_file(format!("{name}.npy"), options)?;
        archive.write_all(&npy(array))?;
    }
    archive.finish()?.sync_all()?;
    std::fs::rename(&temporary, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrays_read_back_as_written() {
        let path = std::env::temp_dir().join(format!("vrft-npz-{}.npz", std::process::id()));
        let arrays = vec![
            (
                "meta".to_string(),
                Array::Text("{\"schema\": \"ü\"}".into()),
            ),
            (
                "w".to_string(),
                Array::Float {
                    shape: vec![2, 3],
                    values: vec![1.0, -2.5, 3.25, 0.0, 1e-7, 6.0],
                },
            ),
            (
                "b".to_string(),
                Array::Float {
                    shape: vec![3],
                    values: vec![0.5, 0.25, -1.0],
                },
            ),
        ];
        write(&path, &arrays).unwrap();
        let read = read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        for (name, array) in &arrays {
            match (array, &read[name]) {
                (Array::Text(a), Array::Text(b)) => assert_eq!(a, b),
                (
                    Array::Float { shape, values },
                    Array::Float {
                        shape: read_shape,
                        values: read_values,
                    },
                ) => assert_eq!((shape, values), (read_shape, read_values)),
                _ => panic!("{name} changed type"),
            }
        }
        let data = 6 * 4;
        assert_eq!(
            (npy(&arrays[1].1).len() - data) % 64,
            0,
            "data starts aligned"
        );
    }
}
