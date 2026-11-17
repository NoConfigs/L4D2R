//! Read Valve VPK directory trees without copying or modifying game data.
use std::{
    fs,
    io::{self, Cursor, Read},
    path::Path,
};

#[derive(Debug)]
pub struct Entry {
    pub path: String,
    pub crc: u32,
    pub archive: u16,
    pub offset: u32,
    pub length: u32,
    pub preload: Vec<u8>,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn u16le(c: &mut Cursor<&[u8]>) -> io::Result<u16> {
    let mut b = [0; 2];
    c.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn u32le(c: &mut Cursor<&[u8]>) -> io::Result<u32> {
    let mut b = [0; 4];
    c.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn string(c: &mut Cursor<&[u8]>) -> io::Result<String> {
    let start = c.position() as usize;
    let bytes = c.get_ref();
    let end = bytes
        .get(start..)
        .and_then(|b| b.iter().position(|v| *v == 0))
        .ok_or_else(|| invalid("Unterminated VPK string"))?
        + start;
    // Directory names are byte strings in Source. This metadata report uses
    // replacement characters for legacy encodings; it is not an extraction key.
    let result = String::from_utf8_lossy(&bytes[start..end]).into_owned();
    c.set_position((end + 1) as u64);
    Ok(result)
}
pub fn parse(bytes: &[u8]) -> io::Result<Vec<Entry>> {
    let mut header = Cursor::new(bytes);
    if u32le(&mut header)? != 0x55aa1234 {
        return Err(invalid("Invalid VPK signature"));
    }
    let version = u32le(&mut header)?;
    let size = u32le(&mut header)? as usize;
    let start: usize = match version {
        1 => 12,
        2 => 28,
        _ => return Err(invalid("Unsupported VPK version")),
    };
    let end = start
        .checked_add(size)
        .ok_or_else(|| invalid("VPK tree overflow"))?;
    let mut c = Cursor::new(
        bytes
            .get(start..end)
            .ok_or_else(|| invalid("Truncated VPK directory tree"))?,
    );
    let mut entries = Vec::new();
    loop {
        let extension = string(&mut c)?;
        if extension.is_empty() {
            break;
        }
        loop {
            let directory = string(&mut c)?;
            if directory.is_empty() {
                break;
            }
            loop {
                let name = string(&mut c)?;
                if name.is_empty() {
                    break;
                }
                let crc = u32le(&mut c)?;
                let preload_length = u16le(&mut c)? as usize;
                let archive = u16le(&mut c)?;
                let offset = u32le(&mut c)?;
                let length = u32le(&mut c)?;
                if u16le(&mut c)? != 0xffff {
                    return Err(invalid("Invalid VPK entry terminator"));
                }
                let mut preload = vec![0; preload_length];
                c.read_exact(&mut preload)?;
                let path = format!(
                    "{}{}{}{}",
                    if directory == " " { "" } else { &directory },
                    if directory == " " { "" } else { "/" },
                    name,
                    if extension == " " {
                        String::new()
                    } else {
                        format!(".{extension}")
                    }
                );
                entries.push(Entry {
                    path,
                    crc,
                    archive,
                    offset,
                    length,
                    preload,
                });
            }
        }
    }
    Ok(entries)
}
pub fn index_installation(root: &Path) -> io::Result<()> {
    if !root.join("left4dead2.exe").is_file() {
        return Err(invalid(
            "Expected an L4D2 installation containing left4dead2.exe",
        ));
    }
    let mut report = String::from(
        "# Local L4D2 VPK inventory — metadata only\n# mount\tpath\tarchive\toffset\tlength\tpreload\tcrc32\n",
    );
    let mut count = 0;
    // Source mounts update/DLC ahead of the base game; inventory records every copy.
    for mount in [
        "update",
        "left4dead2_dlc3",
        "left4dead2_dlc2",
        "left4dead2_dlc1",
        "left4dead2",
    ] {
        let directory = root.join(mount);
        if !directory.is_dir() {
            continue;
        }
        let mut archives: Vec<_> = fs::read_dir(directory)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with("_dir.vpk"))
            })
            .collect();
        archives.sort();
        for archive in archives {
            let entries = parse(&fs::read(&archive)?)?;
            println!("{}: {} entries", archive.display(), entries.len());
            for e in entries {
                report.push_str(&format!(
                    "{mount}/{}\t{}\t{}\t{}\t{}\t{}\t{:08x}\n",
                    archive.file_name().unwrap().to_string_lossy(),
                    e.path,
                    e.archive,
                    e.offset,
                    e.length,
                    e.preload.len(),
                    e.crc
                ));
                count += 1;
            }
        }
    }
    if count == 0 {
        return Err(invalid("No VPK directory archives found"));
    }
    fs::write("asset-index.txt", report)?;
    println!("Indexed {count} entries in asset-index.txt. No game assets were extracted.");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_bad_or_truncated_archives() {
        assert!(parse(&[]).is_err());
        assert!(parse(&[0; 28]).is_err());
        let mut b = Vec::new();
        b.extend(0x55aa1234u32.to_le_bytes());
        b.extend(2u32.to_le_bytes());
        b.extend(999u32.to_le_bytes());
        assert!(parse(&b).is_err());
    }
    #[test]
    fn reads_preload_and_archive_location() {
        let mut tree = b"txt\0 \0hello\0".to_vec();
        tree.extend(42u32.to_le_bytes());
        tree.extend(2u16.to_le_bytes());
        tree.extend(3u16.to_le_bytes());
        tree.extend(100u32.to_le_bytes());
        tree.extend(200u32.to_le_bytes());
        tree.extend(0xffffu16.to_le_bytes());
        tree.extend(b"hi\0\0\0");
        let mut b = Vec::new();
        b.extend(0x55aa1234u32.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend((tree.len() as u32).to_le_bytes());
        b.extend(tree);
        let e = parse(&b).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].path, "hello.txt");
        assert_eq!(e[0].preload, b"hi");
        assert_eq!(e[0].archive, 3);
        assert_eq!(e[0].offset, 100);
        assert_eq!(e[0].length, 200);
    }
}
