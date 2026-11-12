//! The add/edit form of a machine profile: exactly the fields pod-remote
//! stores (docs/17 §2.1), validated by pod-remote's own rules, and saving
//! through its [`MachineStore`] (the file format has one owner).

use std::path::PathBuf;

use pod_remote::machine::{validate_host, validate_name, validate_path, validate_user};
use pod_remote::{Machine, MachineStore};

use super::copy;

/// The six text fields, as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileForm {
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: String,
    pub identity_file: String,
    pub known_hosts: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Host,
    User,
    Port,
    IdentityFile,
    KnownHosts,
}

/// The first invalid field and why (pod-remote's Korean message).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormError {
    pub field: Field,
    pub message: String,
}

fn err(field: Field, message: impl Into<String>) -> FormError {
    FormError {
        field,
        message: message.into(),
    }
}

impl ProfileForm {
    pub fn from_machine(m: &Machine) -> Self {
        let path = |p: &Option<PathBuf>| {
            p.as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        };
        Self {
            name: m.name.clone(),
            host: m.host.clone(),
            user: m.user.clone().unwrap_or_default(),
            port: m.port.map(|p| p.to_string()).unwrap_or_default(),
            identity_file: path(&m.identity_file),
            known_hosts: path(&m.known_hosts),
        }
    }

    /// The profile, or the first field that pod-remote would refuse. Only
    /// surrounding whitespace is trimmed; nothing else is rewritten.
    pub fn to_machine(&self) -> Result<Machine, FormError> {
        let name = self.name.trim();
        validate_name(name).map_err(|e| err(Field::Name, e))?;
        let host = self.host.trim();
        validate_host(host).map_err(|e| err(Field::Host, e))?;
        let user = Some(self.user.trim()).filter(|u| !u.is_empty());
        if let Some(u) = user {
            validate_user(u).map_err(|e| err(Field::User, e))?;
        }
        let port = match self.port.trim() {
            "" => None,
            p => match p.parse::<u16>() {
                Ok(n) if n > 0 && p.bytes().all(|b| b.is_ascii_digit()) => Some(n),
                _ => return Err(err(Field::Port, copy::PORT_INVALID)),
            },
        };
        let path = |field, what, v: &str| -> Result<Option<PathBuf>, FormError> {
            let v = v.trim();
            if v.is_empty() {
                return Ok(None);
            }
            let p = PathBuf::from(v);
            validate_path(what, &p).map_err(|e| err(field, e))?;
            Ok(Some(p))
        };
        let m = Machine {
            name: name.to_string(),
            host: host.to_string(),
            user: user.map(str::to_string),
            port,
            identity_file: path(Field::IdentityFile, "키", &self.identity_file)?,
            known_hosts: path(Field::KnownHosts, "known_hosts", &self.known_hosts)?,
        };
        // The store validates again; keep one rule set.
        m.validate().map_err(|e| err(Field::Name, e))?;
        Ok(m)
    }
}

/// Add `m`, or replace the profile `original` with it (an edit; a rename
/// when the names differ). Nothing is written when `m` is invalid or its
/// name is taken by another profile.
pub fn save(store: &MachineStore, original: Option<&str>, m: Machine) -> Result<(), FormError> {
    m.validate().map_err(|e| err(Field::Name, e))?;
    let all = store.load().map_err(|e| err(Field::Name, e))?;
    if all
        .iter()
        .any(|x| x.name == m.name && Some(x.name.as_str()) != original)
    {
        return Err(err(Field::Name, copy::NAME_TAKEN));
    }
    let Some(old_name) = original else {
        return store.add(m).map_err(|e| err(Field::Name, e));
    };
    let old = all.into_iter().find(|x| x.name == old_name);
    store.remove(old_name).map_err(|e| err(Field::Name, e))?;
    store.add(m).map_err(|e| {
        // Put the previous profile back rather than lose it.
        if let Some(o) = old {
            let _ = store.add(o);
        }
        err(Field::Name, e)
    })
}
