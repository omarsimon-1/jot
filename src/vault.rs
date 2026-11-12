//! Encrypted storage: every entry and media file is sealed with XChaCha20-Poly1305
//! under a random master key, which is itself wrapped by an Argon2id password key.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use argon2::Argon2;
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{stream, Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const CHUNK: usize = 1 << 20;
const TAG: usize = 16;
const IMAGE_EXT: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "heic", "heif", "avif", "bmp", "tif", "tiff"];
const VIDEO_EXT: &[&str] = &["mp4", "mov", "mkv", "webm", "m4v", "avi"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Media {
    pub file: String,
    pub name: String,
    pub video: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub created: DateTime<Local>,
    pub text: String,
    #[serde(default)]
    pub media: Vec<Media>,
    /// Private entries never leave the app through agent access.
    #[serde(default)]
    pub private: bool,
}

impl Entry {
    pub fn date(&self) -> NaiveDate {
        self.created.date_naive()
    }
    pub fn time_label(&self) -> String {
        self.created.format("%-I:%M %p").to_string()
    }
}

#[derive(Serialize, Deserialize)]
struct Meta {
    version: u32,
    salt: String,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
    wrapped_key: String,
    /// Identifies this journal so sync never mixes two separately created ones.
    #[serde(default)]
    id: String,
}

#[derive(Clone)]
pub struct Vault {
    dir: PathBuf,
    key: Arc<Zeroizing<[u8; 32]>>,
}

pub fn default_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("JOT_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    base.join("jot")
}

pub fn media_kind(path: &Path) -> Option<bool> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    if IMAGE_EXT.contains(&ext.as_str()) {
        Some(false)
    } else if VIDEO_EXT.contains(&ext.as_str()) {
        Some(true)
    } else {
        None
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        bail!("bad hex");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| anyhow!("bad hex")))
        .collect()
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

fn derive(password: &str, salt: &[u8], m: u32, t: u32, p: u32) -> Result<Zeroizing<[u8; 32]>> {
    let params = argon2::Params::new(m, t, p, Some(32)).map_err(|e| anyhow!("{e}"))?;
    let a = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    a.hash_password_into(password.as_bytes(), salt, out.as_mut())
        .map_err(|e| anyhow!("{e}"))?;
    Ok(out)
}

fn seal(key: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let mut out = nonce.to_vec();
    out.extend(cipher.encrypt(&nonce, plain).map_err(|_| anyhow!("encryption failed"))?);
    Ok(out)
}

fn unseal(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 24 + TAG {
        bail!("corrupt data");
    }
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(XNonce::from_slice(&data[..24]), &data[24..])
        .map_err(|_| anyhow!("decryption failed"))
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// Chunked authenticated encryption, so large videos never sit in memory whole.
fn encrypt_stream(key: &[u8; 32], mut r: impl Read, mut w: impl Write) -> Result<()> {
    let nonce: [u8; 19] = random();
    w.write_all(&nonce)?;
    let mut enc = stream::EncryptorBE32::from_aead(XChaCha20Poly1305::new(key.into()), nonce.as_ref().into());
    let mut cur = vec![0u8; CHUNK];
    let mut next = vec![0u8; CHUNK];
    let mut n = read_full(&mut r, &mut cur)?;
    loop {
        let m = if n == CHUNK { read_full(&mut r, &mut next)? } else { 0 };
        if m == 0 {
            let ct = enc.encrypt_last(&cur[..n]).map_err(|_| anyhow!("encryption failed"))?;
            w.write_all(&ct)?;
            break;
        }
        let ct = enc.encrypt_next(&cur[..n]).map_err(|_| anyhow!("encryption failed"))?;
        w.write_all(&ct)?;
        std::mem::swap(&mut cur, &mut next);
        n = m;
    }
    w.flush()?;
    Ok(())
}

fn decrypt_stream(key: &[u8; 32], mut r: impl Read, mut w: impl Write) -> Result<()> {
    let mut nonce = [0u8; 19];
    r.read_exact(&mut nonce).context("corrupt media file")?;
    let mut dec = stream::DecryptorBE32::from_aead(XChaCha20Poly1305::new(key.into()), nonce.as_ref().into());
    let full = CHUNK + TAG;
    let mut cur = vec![0u8; full];
    let mut next = vec![0u8; full];
    let mut n = read_full(&mut r, &mut cur)?;
    loop {
        let m = if n == full { read_full(&mut r, &mut next)? } else { 0 };
        if m == 0 {
            let pt = dec.decrypt_last(&cur[..n]).map_err(|_| anyhow!("decryption failed"))?;
            w.write_all(&pt)?;
            break;
        }
        let pt = dec.decrypt_next(&cur[..n]).map_err(|_| anyhow!("decryption failed"))?;
        w.write_all(&pt)?;
        std::mem::swap(&mut cur, &mut next);
        n = m;
    }
    w.flush()?;
    Ok(())
}

fn private_file(path: &Path) -> Result<fs::File> {
    Ok(fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?)
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = private_file(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn private_dir(path: &Path) -> Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    Ok(())
}

impl Vault {
    pub fn exists(dir: &Path) -> bool {
        dir.join("vault.json").exists()
    }

    pub fn create(dir: &Path, password: &str) -> Result<Vault> {
        if Self::exists(dir) {
            bail!("a journal already exists here");
        }
        private_dir(&dir.join("entries"))?;
        private_dir(&dir.join("media"))?;
        let salt: [u8; 16] = random();
        let (m, t, p) = (64 * 1024, 3, 1);
        let pw_key = derive(password, &salt, m, t, p)?;
        let master = Zeroizing::new(random::<32>());
        let meta = Meta {
            version: 1,
            salt: hex(&salt),
            m_cost: m,
            t_cost: t,
            p_cost: p,
            wrapped_key: hex(&seal(&pw_key, master.as_ref())?),
            id: hex(&random::<8>()),
        };
        write_atomic(&dir.join("vault.json"), serde_json::to_string_pretty(&meta)?.as_bytes())?;
        Ok(Vault { dir: dir.to_path_buf(), key: Arc::new(master) })
    }

    pub fn open(dir: &Path, password: &str) -> Result<Vault> {
        let mut meta: Meta = serde_json::from_slice(&fs::read(dir.join("vault.json"))?)?;
        let pw_key = derive(password, &unhex(&meta.salt)?, meta.m_cost, meta.t_cost, meta.p_cost)?;
        let raw = Zeroizing::new(unseal(&pw_key, &unhex(&meta.wrapped_key)?).map_err(|_| anyhow!("Wrong password"))?);
        let mut master = Zeroizing::new([0u8; 32]);
        if raw.len() != 32 {
            bail!("corrupt vault");
        }
        master.copy_from_slice(&raw);
        if meta.id.is_empty() {
            meta.id = hex(&random::<8>());
            write_atomic(&dir.join("vault.json"), serde_json::to_string_pretty(&meta)?.as_bytes())?;
        }
        Ok(Vault { dir: dir.to_path_buf(), key: Arc::new(master) })
    }

    /// Re-wraps the same master key under a new password; entries are untouched.
    pub fn change_password(&self, new_password: &str) -> Result<()> {
        let path = self.dir.join("vault.json");
        let mut meta: Meta = serde_json::from_slice(&fs::read(&path)?)?;
        let salt: [u8; 16] = random();
        let pw_key = derive(new_password, &salt, meta.m_cost, meta.t_cost, meta.p_cost)?;
        meta.salt = hex(&salt);
        meta.wrapped_key = hex(&seal(&pw_key, self.key.as_ref().as_ref())?);
        write_atomic(&path, serde_json::to_string_pretty(&meta)?.as_bytes())
    }

    pub fn load_entries(&self) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        for f in fs::read_dir(self.dir.join("entries"))? {
            let path = f?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("enc") {
                continue;
            }
            let plain = unseal(&self.key, &fs::read(&path)?)
                .with_context(|| format!("could not read {}", path.display()))?;
            out.push(serde_json::from_slice::<Entry>(&plain)?);
        }
        out.sort_by_key(|e| e.created);
        Ok(out)
    }

    pub fn save_entry(&self, e: &Entry) -> Result<()> {
        let data = seal(&self.key, &serde_json::to_vec(e)?)?;
        write_atomic(&self.dir.join("entries").join(format!("{}.enc", e.id)), &data)
    }

    pub fn delete_entry(&self, id: &str) {
        let _ = fs::remove_file(self.dir.join("entries").join(format!("{id}.enc")));
    }

    fn media_path(&self, file: &str) -> Result<PathBuf> {
        if file.contains('/') || file.contains("..") {
            bail!("bad media name");
        }
        Ok(self.dir.join("media").join(format!("{file}.enc")))
    }

    fn new_media(&self, name: &str, ext: &str, video: bool, src: impl Read) -> Result<Media> {
        let file = format!("{}.{}", hex(&random::<8>()), ext);
        let path = self.media_path(&file)?;
        let tmp = path.with_extension("tmp");
        encrypt_stream(&self.key, src, BufWriter::new(private_file(&tmp)?))?;
        fs::rename(&tmp, &path)?;
        Ok(Media { file, name: name.to_string(), video })
    }

    pub fn import_media(&self, src: &Path) -> Result<Media> {
        let video = media_kind(src).ok_or_else(|| anyhow!("not a photo or video"))?;
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("bin").to_lowercase();
        let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("attachment");
        self.new_media(name, &ext, video, BufReader::new(fs::File::open(src)?))
    }

    pub fn import_png(&self, bytes: &[u8]) -> Result<Media> {
        self.new_media("pasted image.png", "png", false, bytes)
    }

    pub fn read_media(&self, file: &str) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        decrypt_stream(&self.key, BufReader::new(fs::File::open(self.media_path(file)?)?), &mut out)?;
        Ok(out)
    }

    pub fn decrypt_media_to(&self, file: &str, dest: &Path) -> Result<()> {
        let src = BufReader::new(fs::File::open(self.media_path(file)?)?);
        decrypt_stream(&self.key, src, BufWriter::new(private_file(dest)?))
    }

    pub fn delete_media(&self, file: &str) {
        if let Ok(p) = self.media_path(file) {
            let _ = fs::remove_file(p);
        }
    }

    /// Writes one plain markdown file per day plus a media folder. Returns the day count.
    pub fn export(&self, entries: &[Entry], dest: &Path) -> Result<usize> {
        fs::create_dir_all(dest.join("media"))?;
        let days = group_by_day(entries);
        for (date, es) in &days {
            fs::write(dest.join(format!("{date}.md")), day_markdown(*date, es))?;
            for m in es.iter().flat_map(|e| &e.media) {
                self.decrypt_media_to(&m.file, &dest.join("media").join(&m.file))?;
            }
        }
        Ok(days.len())
    }
}

fn vault_id(meta_json: &[u8]) -> Option<String> {
    serde_json::from_slice::<Meta>(meta_json).ok().map(|m| m.id)
}

fn vault_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("vault.json")];
    for sub in ["entries", "media"] {
        for f in fs::read_dir(dir.join(sub)).into_iter().flatten().flatten() {
            let name = f.file_name();
            if Path::new(&name).extension().is_some_and(|e| e == "enc") {
                out.push(Path::new(sub).join(name));
            }
        }
    }
    out
}

/// Copies the (already encrypted) vault files that are new or changed. Returns how many.
pub fn backup(dir: &Path, dest: &Path) -> Result<usize> {
    if !Vault::exists(dir) {
        bail!("there is no journal to back up yet");
    }
    private_dir(&dest.join("entries"))?;
    private_dir(&dest.join("media"))?;
    let mut copied = 0;
    for rel in vault_files(dir) {
        let (src, dst) = (dir.join(&rel), dest.join(&rel));
        let sm = fs::metadata(&src)?;
        let fresh = fs::metadata(&dst)
            .map(|dm| dm.len() == sm.len() && dm.modified().ok() >= sm.modified().ok())
            .unwrap_or(false);
        if !fresh {
            let tmp = dst.with_extension("tmp");
            fs::copy(&src, &tmp)?;
            fs::rename(&tmp, &dst)?;
            copied += 1;
        }
    }
    Ok(copied)
}

fn run(cmd: &mut std::process::Command, what: &str) -> Result<Vec<u8>> {
    let out = cmd.output().with_context(|| format!("could not run {what}"))?;
    if !out.status.success() {
        bail!("{what} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

/// Two-way sync of the encrypted files with another machine over SSH (for
/// example a tailnet host). Only ciphertext travels; nothing is ever deleted.
pub fn sync_with(dir: &Path, host: &str) -> Result<String> {
    if host.is_empty() || host.starts_with('-') || host.chars().any(|c| c.is_whitespace() || c == '\'' || c == '"') {
        bail!("that does not look like a host name");
    }
    const REMOTE: &str = ".local/share/jot";
    const SSH: &str = "ssh -o BatchMode=yes -o ConnectTimeout=6";
    let ssh = |script: &str| {
        let mut c = std::process::Command::new("ssh");
        c.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=6", host, script]);
        c
    };
    let remote_meta = run(&mut ssh(&format!("cat {REMOTE}/vault.json 2>/dev/null || true")), "ssh")?;
    let remote_id = vault_id(&remote_meta);
    let local_id = fs::read(dir.join("vault.json")).ok().and_then(|b| vault_id(&b));
    match (&local_id, &remote_id) {
        (None, None) => bail!("there is no journal on either machine yet"),
        (Some(l), Some(r)) if l.is_empty() || r.is_empty() => {
            bail!("open Jot once on both machines after updating, then sync again")
        }
        (Some(l), Some(r)) if l != r => {
            bail!("{host} holds a different journal that was created separately; not mixing them")
        }
        _ => {}
    }
    let rsync = |from: &str, to: &str| {
        let mut c = std::process::Command::new("rsync");
        c.args(["-a", "--update", "--exclude", "*.tmp", "--chmod=D700,F600", "-e", SSH, from, to]);
        run(&mut c, "rsync").map(|_| ())
    };
    let local = format!("{}/", dir.display());
    let remote = format!("{host}:{REMOTE}/");
    let before = vault_files(dir).len();
    if remote_id.is_some() {
        private_dir(dir)?;
        rsync(&remote, &local)?;
    }
    let pulled = vault_files(dir).len().saturating_sub(if local_id.is_some() { before } else { 0 });
    if local_id.is_some() {
        run(&mut ssh(&format!("mkdir -p -m 700 {REMOTE}")), "ssh")?;
        rsync(&local, &remote)?;
    }
    Ok(format!("Synced with {host}, {pulled} new file(s) received"))
}

pub fn group_by_day(entries: &[Entry]) -> BTreeMap<NaiveDate, Vec<&Entry>> {
    let mut days: BTreeMap<NaiveDate, Vec<&Entry>> = BTreeMap::new();
    for e in entries {
        days.entry(e.date()).or_default().push(e);
    }
    days
}

pub fn day_markdown(date: NaiveDate, entries: &[&Entry]) -> String {
    let mut out = format!("# {}\n", date.format("%A, %B %-d, %Y"));
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            out.push_str("\n---\n");
        }
        out.push_str(&format!("\n###### {}\n", e.time_label()));
        if !e.text.trim().is_empty() {
            out.push_str(&format!("\n{}\n", e.text.trim_end()));
        }
        for m in &e.media {
            out.push_str(&format!("\n![{}](media/{})\n", m.name, m.file));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("journal-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn roundtrip_and_wrong_password() {
        let d = tmp("vault");
        let v = Vault::create(&d, "hunter2").unwrap();
        let e = Entry { id: "a".into(), created: Local::now(), text: "hello **world**".into(), media: vec![], private: false };
        v.save_entry(&e).unwrap();
        assert!(Vault::open(&d, "nope").is_err());
        let v2 = Vault::open(&d, "hunter2").unwrap();
        let got = v2.load_entries().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, e.text);
        let raw = fs::read(d.join("entries/a.enc")).unwrap();
        assert!(!raw.windows(5).any(|w| w == b"hello"));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn password_change_and_backup() {
        let d = tmp("pwchange");
        let v = Vault::create(&d, "old").unwrap();
        let e = Entry { id: "a".into(), created: Local::now(), text: "kept".into(), media: vec![], private: false };
        v.save_entry(&e).unwrap();
        v.change_password("new").unwrap();
        assert!(Vault::open(&d, "old").is_err());
        assert_eq!(Vault::open(&d, "new").unwrap().load_entries().unwrap()[0].text, "kept");

        let b = d.join("backup");
        assert_eq!(backup(&d, &b).unwrap(), 2);
        assert_eq!(backup(&d, &b).unwrap(), 0);
        assert_eq!(Vault::open(&b, "new").unwrap().load_entries().unwrap().len(), 1);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn media_stream_sizes_and_export() {
        let d = tmp("media");
        let v = Vault::create(&d, "pw").unwrap();
        for size in [0usize, 1, CHUNK - 1, CHUNK, CHUNK + 1, 2 * CHUNK, 2 * CHUNK + 7] {
            let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let src = d.join("clip.mp4");
            fs::write(&src, &data).unwrap();
            let m = v.import_media(&src).unwrap();
            assert!(m.video);
            assert_eq!(v.read_media(&m.file).unwrap(), data, "size {size}");
        }
        let m = v.import_png(b"not really a png").unwrap();
        let e = Entry { id: "b".into(), created: Local::now(), text: "note".into(), media: vec![m.clone()], private: false };
        let out = d.join("out");
        assert_eq!(v.export(&[e], &out).unwrap(), 1);
        assert_eq!(fs::read(out.join("media").join(&m.file)).unwrap(), b"not really a png");
        let md = fs::read_to_string(out.join(format!("{}.md", Local::now().date_naive()))).unwrap();
        assert!(md.contains("note") && md.contains(&format!("media/{}", m.file)));
        fs::remove_dir_all(&d).unwrap();
    }

    /// Builds a throwaway demo vault at $JOT_DIR (password "demo") for visual checks.
    #[test]
    #[ignore]
    fn seed_demo() {
        use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
        let d = default_dir();
        let _ = fs::remove_dir_all(&d);
        let v = Vault::create(&d, "demo").unwrap();
        let pb = Pixbuf::new(Colorspace::Rgb, false, 8, 900, 500).unwrap();
        pb.fill(0x8c8c8cff);
        let png = pb.save_to_bufferv("png", &[]).unwrap();
        let photo = v.import_png(&png).unwrap();
        fs::write(d.join("clip.mp4"), b"not a real video").unwrap();
        let clip = v.import_media(&d.join("clip.mp4")).unwrap();
        let now = Local::now();
        let mk = |id: &str, ago_min: i64, text: &str, media: Vec<Media>| Entry {
            id: id.into(),
            created: now - chrono::Duration::minutes(ago_min),
            text: text.into(),
            media,
            private: false,
        };
        let entries = [
            mk("a", 60 * 26, "Yesterday's note about the **harbour** walk.", vec![]),
            mk("b", 95, "# Morning\nCoffee on the porch. The light was *unreasonably* good, and I finally fixed the ~~gate~~ fence.\n> Write it down before it goes.\n- call Sam\n- book the `ferry`", vec![photo]),
            mk("d", 525_600, "A year ago today: first frost, and the long drive north.", vec![]),
            mk("c", 20, "Second entry of the day, with a clip.", vec![clip]),
        ];
        for e in &entries {
            v.save_entry(e).unwrap();
        }
    }
}
