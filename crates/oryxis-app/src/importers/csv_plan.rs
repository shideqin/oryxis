//! What each row of an imported CSV does to the vault. The reader
//! ([`super::csv`]) only records cells; this half holds them against
//! the hosts, folders, identities and keys the vault has, so the
//! preview can say, row by row, what the save will do:
//!
//! - a row with no `id`, or one that names no host here (a file from
//!   another vault), ADDS a host, with a fresh id (a foreign id reused
//!   here would merge two hosts the next time the vaults sync);
//! - a row whose `id` is a host here UPDATES it: the columns the file
//!   has are laid over the host as it is, every other field stays;
//! - a row whose `action` is `remove` REMOVES the host its `id` names,
//!   or, with no id, the one host carrying its label.
//!
//! Nothing is guessed. A row that cannot be read one way (an id that
//! is not one, a label two hosts share, an identity nobody has, a
//! folder that is a dynamic one) does nothing and says why.

use std::collections::HashSet;

use oryxis_core::models::connection::{AuthMethod, Connection, ConnectionProtocol};
use oryxis_core::models::{Group, Identity, SshKey};
use uuid::Uuid;

use super::csv::{protocol_token, Column, SheetRow};
use super::{DirectHost, DirectImport, FolderPlan, HostOp};

/// What the vault already holds, as the planner reads it.
pub(crate) struct Known<'a> {
    pub connections: &'a [Connection],
    pub groups: &'a [Group],
    pub identities: &'a [Identity],
    pub keys: &'a [SshKey],
}

/// Why a row does nothing: the row's name, an i18n key and the value
/// the reason is about (substituted for `{value}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub name: String,
    pub reason: &'static str,
    pub value: Option<String>,
}

impl Refusal {
    fn new(row: &SheetRow, reason: &'static str, value: Option<&str>) -> Self {
        Self {
            name: row.name(),
            reason,
            value: value.map(str::to_string),
        }
    }

    /// `"name (reason)"`, the shape of every other "not importable"
    /// entry.
    pub(crate) fn render(&self) -> String {
        let reason = crate::i18n::t(self.reason);
        let reason = match &self.value {
            Some(v) => reason.replace("{value}", v),
            None => reason.to_string(),
        };
        format!("{} ({reason})", self.name)
    }
}

/// Plan every CSV row of a batch; rows from any other source pass
/// through untouched. A row that cannot be planned joins `skipped`
/// with its reason.
pub(crate) fn plan(mut import: DirectImport, known: &Known<'_>) -> DirectImport {
    let mut claimed: HashSet<Uuid> = HashSet::new();
    for mut host in std::mem::take(&mut import.hosts) {
        let Some(sheet) = host.sheet.take() else {
            import.hosts.push(host);
            continue;
        };
        match plan_row(&sheet, host.password.take(), known, &mut claimed) {
            Ok(planned) => import.hosts.push(planned),
            Err(refusal) => import.skipped.push(refusal.render()),
        }
    }
    import
}

/// One row. `claimed` holds the known hosts earlier rows already
/// update or remove: a second row about the same host is refused, so
/// the file cannot say two things about one host and have the later
/// one win in silence.
pub(crate) fn plan_row(
    row: &SheetRow,
    password: Option<String>,
    known: &Known<'_>,
    claimed: &mut HashSet<Uuid>,
) -> Result<DirectHost, Refusal> {
    let action = row.text(Column::Action);
    let remove = match action.to_ascii_lowercase().as_str() {
        "" | "add" | "update" | "upsert" => false,
        "remove" | "delete" => true,
        _ => return Err(Refusal::new(row, "import_refused_action", Some(action))),
    };
    // `id` is the most generic header there is: an inventory sheet's
    // `1`, `2`, `srv-17` are not vault ids and carry no instruction,
    // so the row reads as having none. Only a cell shaped like the ids
    // the export writes (8-4-4-4-12) and still unreadable is a typo of
    // one, refused rather than turned into a duplicate.
    let id = match row.text(Column::Id) {
        "" => None,
        raw => match Uuid::parse_str(raw) {
            Ok(id) => Some(id),
            Err(_) if looks_like_uuid(raw) => {
                return Err(Refusal::new(row, "import_refused_bad_id", Some(raw)));
            }
            Err(_) => None,
        },
    };
    let known_host = id.and_then(|id| known.connections.iter().find(|c| c.id == id));
    // A host is claimed only by a row that plans: one refused for its
    // own reason (an identity nobody has) leaves the host free for a
    // later row, which is then not "the second time".
    let twice = |conn: &Connection| {
        if claimed.contains(&conn.id) {
            Err(Refusal::new(row, "import_refused_twice", None))
        } else {
            Ok(())
        }
    };

    let planned = if remove {
        let target = match (id, known_host) {
            (_, Some(conn)) => conn,
            (Some(_), None) => return Err(Refusal::new(row, "import_refused_no_such_id", None)),
            (None, None) => host_by_label(row, known.connections)?,
        };
        twice(target)?;
        DirectHost {
            conn: target.clone(),
            password: None,
            op: HostOp::Remove,
            folder: FolderPlan::Keep,
            sheet: None,
        }
    } else {
        match known_host {
            Some(existing) => {
                twice(existing)?;
                plan_update(row, existing, password, known)?
            }
            None => return plan_add(row, password, known),
        }
    };
    claimed.insert(planned.conn.id);
    Ok(planned)
}

/// The 8-4-4-4-12 hex shape of the ids [`super::csv::render`] writes.
fn looks_like_uuid(raw: &str) -> bool {
    let groups: Vec<&str> = raw.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|g| g.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// The one host a removal names by label. When several share it, the
/// row's address (if it has one) may tell them apart; anything still
/// ambiguous is refused rather than picked.
fn host_by_label<'a>(row: &SheetRow, connections: &'a [Connection]) -> Result<&'a Connection, Refusal> {
    let label = row.text(Column::Label);
    if label.is_empty() {
        return Err(Refusal::new(row, "import_refused_names_nothing", None));
    }
    let mut hits: Vec<&Connection> = connections.iter().filter(|c| c.label == label).collect();
    let hostname = row.text(Column::Hostname);
    if hits.len() > 1 && !hostname.is_empty() {
        hits.retain(|c| c.hostname == hostname);
    }
    match hits.as_slice() {
        [one] => Ok(one),
        [] => Err(Refusal::new(row, "import_refused_no_such_label", None)),
        _ => Err(Refusal::new(row, "import_refused_label_ambiguous", None)),
    }
}

fn plan_add(row: &SheetRow, password: Option<String>, known: &Known<'_>) -> Result<DirectHost, Refusal> {
    let label = row.text(Column::Label);
    let hostname = row.text(Column::Hostname);
    // Either column can stand in for the other, which is what makes a
    // two-column sheet (name,address) and a one-column list of
    // hostnames both work.
    let (label, hostname) = match (label.is_empty(), hostname.is_empty()) {
        (false, false) => (label, hostname),
        (true, false) => (hostname, hostname),
        (false, true) => (label, label),
        (true, true) => return Err(Refusal::new(row, "import_refused_no_address", None)),
    };
    let protocol = match row.text(Column::Protocol) {
        "" => ConnectionProtocol::Ssh,
        token => importable_protocol(token)
            .ok_or_else(|| Refusal::new(row, "import_refused_protocol", Some(token)))?,
    };
    let mut conn = Connection::new(label, hostname);
    conn.protocol = protocol;
    conn.port = match row.text(Column::Port) {
        "" => protocol.default_port().unwrap_or(22),
        raw => parse_port(row, raw)?,
    };
    conn.username = Some(row.text(Column::Username).to_string()).filter(|u| !u.is_empty());
    conn.tags = split_tags(row.text(Column::Tags));
    conn.notes = Some(row.text(Column::Notes).to_string()).filter(|n| !n.is_empty());
    if let Some(name) = row.cell(Column::Identity).filter(|v| !v.is_empty()) {
        conn.identity_id = Some(identity_named(row, name, known.identities)?);
    }
    if let Some(name) = row.cell(Column::Key).filter(|v| !v.is_empty()) {
        conn.key_id = Some(key_named(row, name, known.keys)?);
    }
    // A password the sheet carries is the way in, unless the row also
    // names a key, which `Auto` tries first.
    if password.is_some() && conn.key_id.is_none() {
        conn.auth_method = AuthMethod::Password;
    }
    super::split_host_field(&mut conn);
    let folder = folder_plan(row, row.text(Column::Group), known.groups, FolderPlan::Open)?;
    Ok(DirectHost {
        conn,
        password,
        op: HostOp::Add,
        folder,
        sheet: None,
    })
}

/// Lay the row over `existing`. A column the file does not have leaves
/// its field alone; a column it has with an empty cell clears the
/// field, except the ones a host cannot be without (label, address,
/// port, protocol), where empty means "as it is". An empty password
/// never clears the stored one: a blank cell is not a decision to
/// forget a secret.
fn plan_update(
    row: &SheetRow,
    existing: &Connection,
    password: Option<String>,
    known: &Known<'_>,
) -> Result<DirectHost, Refusal> {
    let mut conn = existing.clone();
    // Cells reach here trimmed, so a value equal to the field TRIMMED
    // is the field as it is (a note the export wrote with a trailing
    // newline must not read back as an edit).
    let same = |cell: &str, field: &str| cell == field.trim();
    if let Some(v) = row.cell(Column::Label).filter(|v| !v.is_empty())
        && !same(v, &existing.label)
    {
        conn.label = v.to_string();
    }
    if let Some(v) = row.cell(Column::Username) {
        conn.username = Some(v.to_string()).filter(|u| !u.is_empty());
    }
    if let Some(v) = row.cell(Column::Protocol).filter(|v| !v.is_empty()) {
        // The token the export wrote for this host keeps it whatever it
        // is (an RDP host exports `rdp`, which no ADD may ask for);
        // a different one may only move between the protocols an add
        // can create.
        if !v.eq_ignore_ascii_case(protocol_token(existing.protocol)) {
            conn.protocol = importable_protocol(v)
                .ok_or_else(|| Refusal::new(row, "import_refused_protocol", Some(v)))?;
        }
    }
    // The port the export wrote for this host keeps it even where no
    // add could ask for it (a serial line's 0).
    if let Some(v) = row.cell(Column::Port).filter(|v| !v.is_empty())
        && *v != existing.port.to_string()
    {
        conn.port = parse_port(row, v)?;
    }
    if let Some(v) = row.cell(Column::Hostname).filter(|v| !v.is_empty())
        && v != existing.hostname
    {
        conn.hostname = v.to_string();
        super::split_host_field(&mut conn);
    }
    if let Some(v) = row.cell(Column::Tags) {
        conn.tags = split_tags(v);
    }
    if let Some(v) = row.cell(Column::Notes)
        && !same(v, existing.notes.as_deref().unwrap_or_default())
    {
        conn.notes = Some(v.to_string()).filter(|n| !n.is_empty());
    }
    if let Some(v) = row.cell(Column::Identity) {
        let current = existing
            .identity_id
            .and_then(|id| known.identities.iter().find(|i| i.id == id));
        conn.identity_id = match v {
            "" => None,
            // The label the export wrote names the identity the host
            // has, even when another identity shares it.
            name if current.is_some_and(|i| i.label == name) => existing.identity_id,
            name => Some(identity_named(row, name, known.identities)?),
        };
    }
    if let Some(v) = row.cell(Column::Key) {
        let current = existing.key_id.and_then(|id| known.keys.iter().find(|k| k.id == id));
        conn.key_id = match v {
            "" => None,
            name if current.is_some_and(|k| k.label == name) => existing.key_id,
            name => Some(key_named(row, name, known.keys)?),
        };
    }
    let folder = match row.cell(Column::Group) {
        None => FolderPlan::Keep,
        Some(v) => folder_plan(row, v, known.groups, FolderPlan::Root)?,
    };
    conn.mark_customized_since(existing);

    let mut changes: Vec<&'static str> = Vec::new();
    let mut note = |name: &'static str, differs: bool| {
        if differs {
            changes.push(name);
        }
    };
    note("label", conn.label != existing.label);
    note("hostname", conn.hostname != existing.hostname);
    note("port", conn.port != existing.port);
    note("username", conn.username != existing.username);
    note("protocol", conn.protocol != existing.protocol);
    note(
        "group",
        match &folder {
            FolderPlan::Open | FolderPlan::Keep => false,
            FolderPlan::Root => existing.group_id.is_some(),
            FolderPlan::Existing(gid) => existing.group_id != Some(*gid),
            FolderPlan::Create(_) => true,
        },
    );
    note("identity", conn.identity_id != existing.identity_id);
    note("key", conn.key_id != existing.key_id);
    note("tags", conn.tags != existing.tags);
    note("notes", conn.notes != existing.notes);
    note("password", password.is_some());
    Ok(DirectHost {
        conn,
        password,
        op: HostOp::Update { changes },
        folder,
        sheet: None,
    })
}

/// The protocols an import can create: the ones a row of host, port
/// and user describes whole. The others need settings no column
/// carries (a serial line, a remote-desktop client, a local shell).
fn importable_protocol(token: &str) -> Option<ConnectionProtocol> {
    match token.to_ascii_lowercase().as_str() {
        "ssh" | "sftp" | "scp" => Some(ConnectionProtocol::Ssh),
        "telnet" => Some(ConnectionProtocol::Telnet),
        _ => None,
    }
}

fn parse_port(row: &SheetRow, raw: &str) -> Result<u16, Refusal> {
    raw.parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| Refusal::new(row, "import_refused_port", Some(raw)))
}

fn split_tags(cell: &str) -> Vec<String> {
    cell.split([',', ';'])
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn identity_named(row: &SheetRow, name: &str, identities: &[Identity]) -> Result<Uuid, Refusal> {
    let hits: Vec<Uuid> = identities.iter().filter(|i| i.label == name).map(|i| i.id).collect();
    match hits.as_slice() {
        [one] => Ok(*one),
        [] => Err(Refusal::new(row, "import_refused_identity_missing", Some(name))),
        _ => Err(Refusal::new(row, "import_refused_identity_ambiguous", Some(name))),
    }
}

fn key_named(row: &SheetRow, name: &str, keys: &[SshKey]) -> Result<Uuid, Refusal> {
    let hits: Vec<Uuid> = keys.iter().filter(|k| k.label == name).map(|k| k.id).collect();
    match hits.as_slice() {
        [one] => Ok(*one),
        [] => Err(Refusal::new(row, "import_refused_key_missing", Some(name))),
        _ => Err(Refusal::new(row, "import_refused_key_ambiguous", Some(name))),
    }
}

/// Where the `group` cell sends the row. `empty` is what a blank cell
/// means: the open folder for a new host, the top level for an update
/// (an export writes a blank for a host at the top level).
///
/// A full path ("Prod / Web", the export's form; "Prod/Web" reads the
/// same) names a folder from the top. A bare name that is no top-level
/// folder may name a nested one, when exactly one folder carries it.
/// A path no folder has is created at the save. A dynamic folder holds
/// no hosts, so a path into one is refused rather than given a manual
/// twin.
fn folder_plan(row: &SheetRow, cell: &str, groups: &[Group], empty: FolderPlan) -> Result<FolderPlan, Refusal> {
    let raw = cell.trim();
    let normalized = raw
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    if normalized.is_empty() {
        return Ok(empty);
    }
    let manual = || groups.iter().filter(|g| g.cloud_query.is_none());
    // The raw form first: a folder whose own name has a `/` in it is
    // found by its path as written.
    for candidate in [raw, normalized.as_str()] {
        let hits: Vec<Uuid> = manual()
            .filter(|g| Group::path_of(groups, g.id) == candidate)
            .map(|g| g.id)
            .collect();
        match hits.as_slice() {
            [one] => return Ok(FolderPlan::Existing(*one)),
            [] => {}
            _ => return Err(Refusal::new(row, "import_refused_folder_ambiguous", Some(raw))),
        }
    }
    let into_dynamic = groups.iter().filter(|g| g.cloud_query.is_some()).any(|g| {
        let path = Group::path_of(groups, g.id);
        normalized == path || normalized.starts_with(&format!("{path} / "))
    });
    if into_dynamic {
        return Err(Refusal::new(row, "import_refused_folder_dynamic", Some(raw)));
    }
    if !normalized.contains(" / ") {
        let hits: Vec<Uuid> = manual().filter(|g| g.label == normalized).map(|g| g.id).collect();
        match hits.as_slice() {
            [one] => return Ok(FolderPlan::Existing(*one)),
            [] => {}
            _ => return Err(Refusal::new(row, "import_refused_folder_ambiguous", Some(raw))),
        }
    }
    Ok(FolderPlan::Create(normalized))
}

/// The folders a save of the ticked rows would create, as full paths,
/// in creation order: what the preview names before anything is made.
pub(crate) fn folders_to_create(groups: &[Group], hosts: &[DirectHost], ticked: &[bool]) -> Vec<String> {
    let mut scratch = groups.to_vec();
    let mut created: Vec<Group> = Vec::new();
    for (host, on) in hosts.iter().zip(ticked) {
        if let (true, FolderPlan::Create(path)) = (on, &host.folder) {
            Group::create_path(&mut scratch, path, &mut created);
        }
    }
    created.iter().map(|g| Group::path_of(&scratch, g.id)).collect()
}
