//! Read the macOS Contacts (AddressBook) SQLite stores and resolve handles to names.
use anyhow::Result;
use imsg_core::{normalize_address, Contact};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{info, warn};

#[derive(Default, Clone)]
pub struct ContactBook {
    pub contacts: Vec<Contact>,
    /// normalized address -> index into `contacts`
    index: HashMap<String, usize>,
    photos: HashMap<i64, Vec<u8>>,
}

impl ContactBook {
    pub fn lookup(&self, address: &str) -> Option<&Contact> {
        let key = normalize_address(address);
        if key.is_empty() {
            return None;
        }
        if let Some(i) = self.index.get(&key) {
            return self.contacts.get(*i);
        }
        // Phones: try suffix match on last 10 digits (handles country codes / local formats).
        if !key.contains('@') && key.len() > 7 {
            let tail = &key[key.len().saturating_sub(10)..];
            if let Some(i) = self.index.get(tail) {
                return self.contacts.get(*i);
            }
        }
        None
    }

    pub fn photo(&self, id: i64) -> Option<&Vec<u8>> {
        self.photos.get(&id)
    }

}

fn store_paths() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let base = PathBuf::from(home).join("Library/Application Support/AddressBook");
    let mut out = vec![];
    let main = base.join("AddressBook-v22.abcddb");
    if main.exists() {
        out.push(main);
    }
    if let Ok(rd) = std::fs::read_dir(base.join("Sources")) {
        for e in rd.flatten() {
            let p = e.path().join("AddressBook-v22.abcddb");
            if p.exists() {
                out.push(p);
            }
        }
    }
    out
}

pub fn load() -> Result<ContactBook> {
    let mut book = ContactBook::default();
    let mut next_id: i64 = 1;
    for path in store_paths() {
        match load_store(&path, &mut book, &mut next_id) {
            Ok(n) => info!("contacts: loaded {n} from {}", path.display()),
            Err(e) => warn!("contacts: failed {}: {e}", path.display()),
        }
    }
    book.contacts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    // Rebuild the index after sorting.
    let mut index = HashMap::new();
    for (i, c) in book.contacts.iter().enumerate() {
        for p in &c.phones {
            index.entry(normalize_address(p)).or_insert(i);
        }
        for e in &c.emails {
            index.entry(normalize_address(e)).or_insert(i);
        }
    }
    book.index = index;
    Ok(book)
}

fn load_store(path: &PathBuf, book: &mut ContactBook, next_id: &mut i64) -> Result<usize> {
    let uri = format!("file:{}?immutable=1", path.display());
    let conn = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut phones: HashMap<i64, Vec<String>> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT ZOWNER, ZFULLNUMBER FROM ZABCDPHONENUMBER WHERE ZFULLNUMBER IS NOT NULL")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for (owner, num) in rows.flatten() {
            phones.entry(owner).or_default().push(num);
        }
    }
    let mut emails: HashMap<i64, Vec<String>> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT ZOWNER, ZADDRESS FROM ZABCDEMAILADDRESS WHERE ZADDRESS IS NOT NULL")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for (owner, addr) in rows.flatten() {
            emails.entry(owner).or_default().push(addr);
        }
    }
    let mut st = conn.prepare(
        "SELECT Z_PK, ZFIRSTNAME, ZLASTNAME, ZORGANIZATION, ZNICKNAME, ZTHUMBNAILIMAGEDATA \
         FROM ZABCDRECORD WHERE \
         ZFIRSTNAME IS NOT NULL OR ZLASTNAME IS NOT NULL OR ZORGANIZATION IS NOT NULL OR ZNICKNAME IS NOT NULL",
    )?;
    let mut count = 0;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<Vec<u8>>>(5)?,
        ))
    })?;
    let mut seen_pk = std::collections::HashSet::new();
    for (pk, first, last, org, nick, photo) in rows.flatten() {
        if !seen_pk.insert(pk) {
            continue;
        }
        let ph = phones.remove(&pk).unwrap_or_default();
        let em = emails.remove(&pk).unwrap_or_default();
        if ph.is_empty() && em.is_empty() {
            continue;
        }
        let mut name = format!(
            "{} {}",
            first.clone().unwrap_or_default().trim(),
            last.clone().unwrap_or_default().trim()
        )
        .trim()
        .to_string();
        if name.is_empty() {
            name = nick.clone().or(org.clone()).unwrap_or_default();
        }
        if name.is_empty() {
            continue;
        }
        let id = *next_id;
        *next_id += 1;
        if let Some(p) = photo {
            if !p.is_empty() {
                book.photos.insert(id, p);
            }
        }
        book.contacts.push(Contact {
            id,
            name,
            first,
            last,
            organization: org,
            phones: ph,
            emails: em,
            has_photo: book.photos.contains_key(&id),
        });
        count += 1;
    }
    Ok(count)
}
