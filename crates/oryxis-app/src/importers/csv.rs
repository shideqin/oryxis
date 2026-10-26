//! CSV / Termius importer, and the CSV host EXPORT that mirrors it:
//! one header-mapped table reader serving both the generic
//! "spreadsheet of hosts" case and Termius' own export (which is a
//! CSV of hosts without secrets), plus [`render`], which writes the
//! same columns back so an exported file re-imports losslessly here
//! and lands recognizably in any tool that reads host spreadsheets.
//!
//! Columns are matched by header NAME, case- and separator-
//! insensitive (`Host Name`, `hostname`, `host_name` all hit the
//! same slot), so a hand-made sheet and a vendor export read the
//! same way and column ORDER never matters.
//!
//! This file only READS: every row becomes a [`SheetRow`] holding the
//! cells as written, and [`super::csv_plan::plan`] decides against the
//! vault what each one does (a new host, an update of the host its
//! `id` names, a removal), which is the half that needs the vault's
//! hosts, folders, identities and keys.
//!
//! Secrets: a `password` column is honored because a user who typed
//! their passwords into a spreadsheet clearly wants them imported,
//! and they land in the vault's encrypted column. Termius' own export
//! carries none, so that path simply never populates it, and
//! [`render`] never writes one.

use oryxis_core::models::connection::Connection;

use super::{DirectHost, DirectImport};

/// Which field a header maps to. Anything unrecognized is ignored
/// rather than guessed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Column {
    /// The host's vault id, as [`render`] writes it: what lets a row
    /// update or remove the host it came from.
    Id,
    /// `add` / `update` (or nothing) and `remove`.
    Action,
    Label,
    Hostname,
    Port,
    Username,
    Password,
    Group,
    /// An identity named by its exact label.
    Identity,
    /// A key named by its exact label.
    Key,
    Tags,
    Notes,
    Protocol,
    Ignored,
}

/// Header aliases, in the normalized form (lowercase, only
/// alphanumerics kept). Termius' export headers are included next to
/// the obvious generic ones.
pub(crate) fn column_of(header: &str) -> Column {
    let key: String = header
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    match key.as_str() {
        "id" | "uuid" | "hostid" => Column::Id,
        "action" | "op" | "operation" => Column::Action,
        "label" | "name" | "alias" | "title" | "sessionname" => Column::Label,
        "hostname" | "host" | "address" | "ip" | "hostaddress" => Column::Hostname,
        "port" | "sshport" => Column::Port,
        "username" | "user" | "login" | "sshusername" => Column::Username,
        "password" | "sshpassword" => Column::Password,
        "group" | "folder" | "groupname" | "path" => Column::Group,
        "identity" => Column::Identity,
        // Only the export's own word: other tools' key columns
        // ("SSH Key", a key file path) mean something else, and a name
        // no key here carries refuses the row.
        "key" => Column::Key,
        "tags" | "labels" => Column::Tags,
        "notes" | "note" | "comment" | "description" => Column::Notes,
        "protocol" | "type" => Column::Protocol,
        _ => Column::Ignored,
    }
}

/// One data row of the table, as written. Only the columns the HEADER
/// carries are here, which is the distinction an update needs: a
/// column the file does not have leaves the host's field alone, a
/// column it has with an empty cell clears it. The password is not
/// kept here: it rides [`DirectHost::password`], whose `Debug` never
/// prints it.
#[derive(Clone, Debug)]
pub(crate) struct SheetRow {
    /// Line in the file (the header is line 1), to name a row that has
    /// nothing else to be named by.
    pub line: usize,
    cells: Vec<(Column, String)>,
}

impl SheetRow {
    /// The trimmed cell of `column`; `None` when the header has no
    /// such column.
    pub(crate) fn cell(&self, column: Column) -> Option<&str> {
        self.cells
            .iter()
            .find(|(c, _)| *c == column)
            .map(|(_, v)| v.as_str())
    }

    /// The cell of `column`, `""` when the column is absent or empty.
    pub(crate) fn text(&self, column: Column) -> &str {
        self.cell(column).unwrap_or_default()
    }

    /// What the row is called in a refusal: its label, else its
    /// address, else its line.
    pub(crate) fn name(&self) -> String {
        [Column::Label, Column::Hostname]
            .into_iter()
            .map(|c| self.text(c))
            .find(|v| !v.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("line {}", self.line))
    }
}

/// Read a CSV table into rows waiting for [`super::csv_plan::plan`].
/// Returns `None` when the header maps to nothing that can name a
/// host, which is what keeps detection from claiming arbitrary text
/// files: an address or a label column, or an `id` next to some other
/// host column (a removal list is `id,action`, a bulk move `id,group`).
pub(crate) fn parse(text: &str) -> Option<DirectImport> {
    let mut rows = read_rows(text);
    if rows.is_empty() {
        return None;
    }
    let header = rows.remove(0);
    let columns: Vec<Column> = header.iter().map(|h| column_of(h)).collect();
    let has = |c: Column| columns.contains(&c);
    let id_and_more = has(Column::Id) && columns.iter().any(|c| !matches!(c, Column::Id | Column::Ignored));
    if !(has(Column::Hostname) || has(Column::Label) || id_and_more) {
        return None;
    }

    let mut out = DirectImport {
        source_key: "import_csv_btn",
        hosts: Vec::new(),
        skipped: Vec::new(),
    };
    for (line_no, row) in rows.iter().enumerate() {
        if row.iter().all(|cell| cell.trim().is_empty()) {
            continue;
        }
        let mut cells: Vec<(Column, String)> = Vec::new();
        let mut password = None;
        for (i, column) in columns.iter().enumerate() {
            // The first column of a kind wins, as a reader of the
            // sheet would assume.
            if *column == Column::Ignored || cells.iter().any(|(c, _)| c == column) {
                continue;
            }
            let value = row.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
            if *column == Column::Password {
                if password.is_none() {
                    password = Some(value).filter(|p| !p.is_empty());
                }
                continue;
            }
            cells.push((*column, value));
        }
        let sheet = SheetRow { line: line_no + 2, cells };
        let mut host = DirectHost::new(Connection::new(sheet.name(), sheet.text(Column::Hostname)), password);
        host.sheet = Some(Box::new(sheet));
        out.hosts.push(host);
    }
    (!out.hosts.is_empty()).then_some(out)
}

/// One host as [`render`] writes it. The folder path, identity and key
/// arrive as LABELS ("Prod / Web"; empty for none) because vault ids
/// mean nothing to the person editing the sheet; the importer reads
/// them back by label.
pub(crate) struct ExportRow<'a> {
    pub conn: &'a Connection,
    pub group: String,
    pub identity: String,
    pub key: String,
}

/// The header [`render`] writes, which is also the set of columns an
/// update of a known host can touch.
pub(crate) const EXPORT_HEADER: &str =
    "id,label,hostname,port,username,protocol,group,identity,key,tags,notes";

/// Render hosts as the CSV [`parse`] reads back: the same
/// header-mapped columns, RFC 4180 quoting, comma-delimited. The `id`
/// column is what makes the file a way to MAINTAIN the vault: edited
/// and imported back, a row with an id updates that host, a row
/// without one adds a host, and a row whose `action` says `remove`
/// removes the host it names (the `action` column is the person's to
/// add; an export has nothing to say in it).
///
/// There is deliberately NO password column, and the function cannot
/// grow one by accident: it never receives secret material at all.
/// The encrypted portable export stays the only secrets-bearing path;
/// this is the shareable, secrets-free view of the host list.
pub(crate) fn render(rows: &[ExportRow<'_>]) -> String {
    let mut out = format!("{EXPORT_HEADER}\n");
    for row in rows {
        let conn = row.conn;
        let cells = [
            conn.id.to_string(),
            conn.label.clone(),
            conn.hostname.clone(),
            conn.port.to_string(),
            conn.username.clone().unwrap_or_default(),
            // `Local` has no scheme (it names no endpoint); every
            // other protocol reuses its quick-connect token, which is
            // also what re-importing recognizes.
            protocol_token(conn.protocol).to_string(),
            row.group.clone(),
            row.identity.clone(),
            row.key.clone(),
            conn.tags.join(","),
            conn.notes.clone().unwrap_or_default(),
        ];
        let mut first = true;
        for cell in cells {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&quote_field(&cell));
        }
        out.push('\n');
    }
    out
}

/// The token a protocol is written as in the `protocol` column.
pub(crate) fn protocol_token(protocol: oryxis_core::models::connection::ConnectionProtocol) -> &'static str {
    protocol.scheme().unwrap_or("local")
}

/// RFC 4180 field quoting. The semicolon is included so a re-read (or
/// another tool's delimiter sniffing) can never split on one inside a
/// notes field.
fn quote_field(cell: &str) -> String {
    if cell.contains(['"', ',', ';', '\n', '\r']) {
        format!("\"{}\"", cell.replace('"', "\"\""))
    } else {
        cell.to_string()
    }
}

/// Minimal RFC 4180 reader: quoted fields, doubled quotes inside
/// them, embedded newlines, and a comma-or-semicolon delimiter
/// sniffed from the header (European exports use `;`).
fn read_rows(text: &str) -> Vec<Vec<String>> {
    let delimiter = sniff_delimiter(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => in_quotes = true,
            c if c == delimiter => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    // A trailing newline leaves an empty final row.
    rows.retain(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    rows
}

/// Delimiter of the first line: whichever of `,` / `;` / tab appears
/// more often outside quotes.
fn sniff_delimiter(text: &str) -> char {
    let first = text.lines().next().unwrap_or_default();
    let count = |d: char| first.matches(d).count();
    let (mut best, mut best_n) = (',', count(','));
    for d in [';', '\t'] {
        if count(d) > best_n {
            best = d;
            best_n = count(d);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use oryxis_core::models::connection::ConnectionProtocol;

    #[test]
    fn a_table_with_no_usable_header_is_not_a_host_list() {
        assert!(parse("total,average\n1,2\n").is_none());
        assert!(parse("").is_none());
        // An id alone could be any table's first column.
        assert!(parse("id,total\n1,2\n").is_none());
    }

    #[test]
    fn a_removal_list_of_ids_is_a_host_list() {
        let import = parse("id,action\n5b4e0d52-0000-4000-8000-000000000001,remove\n").expect("table parses");
        assert_eq!(import.hosts.len(), 1);
        let row = import.hosts[0].sheet.as_deref().expect("a CSV row carries its cells");
        assert_eq!(row.cell(Column::Action), Some("remove"));
        assert_eq!(row.cell(Column::Label), None);
        assert_eq!(row.name(), "line 2");
    }

    #[test]
    fn the_password_rides_the_host_never_the_row() {
        // The row derives Debug; the password must not be one of its
        // cells, so no print of a row can carry it.
        let import = parse("name,host,password\nweb,web.corp,hunter2\n").unwrap();
        let host = &import.hosts[0];
        assert_eq!(host.password.as_deref(), Some("hunter2"));
        let row = host.sheet.as_deref().unwrap();
        assert!(!format!("{row:?}").contains("hunter2"));
        assert!(!format!("{host:?}").contains("hunter2"));
    }

    #[test]
    fn a_column_the_header_lacks_is_absent_and_an_empty_cell_is_empty() {
        let import = parse("name,host,user\nweb,web.corp,\n").unwrap();
        let row = import.hosts[0].sheet.as_deref().unwrap();
        assert_eq!(row.cell(Column::Username), Some(""));
        assert_eq!(row.cell(Column::Notes), None);
    }

    #[test]
    fn export_never_grows_a_password_column() {
        // Structural, like proxy_password_does_not_leak_into_proxy_column:
        // the header is the contract, and no header may map to the
        // password slot. `render` cannot even receive secrets (its
        // input carries none), so the header check is what would catch
        // a future column addition that changes that.
        let conn = Connection::new("a".to_string(), "a.corp".to_string());
        let csv = render(&[ExportRow { conn: &conn, group: String::new(), identity: String::new(), key: String::new() }]);
        let header = csv.lines().next().unwrap();
        for h in header.split(',') {
            assert!(
                !matches!(column_of(h), Column::Password | Column::Ignored),
                "export header {h:?} maps to the password column or to nothing"
            );
        }
        assert_eq!(header, EXPORT_HEADER);
    }

    #[test]
    fn export_writes_every_protocol_by_its_token() {
        // Non-SSH tokens must be the quick-connect vocabulary: the
        // importer re-reads ssh/telnet and reports the others by name
        // instead of silently guessing.
        let mk = |p: ConnectionProtocol| {
            let mut c = Connection::new("x".to_string(), "x.corp".to_string());
            c.protocol = p;
            c
        };
        let conns = [
            (mk(ConnectionProtocol::Ssh), "ssh"),
            (mk(ConnectionProtocol::Telnet), "telnet"),
            (mk(ConnectionProtocol::Raw), "raw"),
            (mk(ConnectionProtocol::Serial), "serial"),
            (mk(ConnectionProtocol::Local), "local"),
            (mk(ConnectionProtocol::RemoteDesktop), "rdp"),
        ];
        let protocol_at = EXPORT_HEADER.split(',').position(|h| h == "protocol").unwrap();
        for (conn, token) in &conns {
            let csv = render(&[ExportRow { conn, group: String::new(), identity: String::new(), key: String::new() }]);
            let row = csv.lines().nth(1).unwrap();
            assert_eq!(row.split(',').nth(protocol_at), Some(*token));
        }
    }
}
