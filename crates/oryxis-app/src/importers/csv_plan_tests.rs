//! What a CSV row does once held against a vault (`csv_plan`), read
//! end to end: the file goes through the same reader the import hub
//! uses, then through the planner.

use oryxis_core::models::cloud::{
    CloudQuery, CloudQueryKind, CloudRef, CloudResourceType, ConnectionTemplate, TransportKind,
};
use oryxis_core::models::connection::{AuthMethod, Connection, ConnectionProtocol};
use oryxis_core::models::key::KeyAlgorithm;
use oryxis_core::models::{Group, Identity, SshKey};

use super::csv::{self, ExportRow};
use super::csv_plan::{self, Known};
use super::{DirectImport, FolderPlan, HostOp};

#[derive(Default)]
struct Vault {
    connections: Vec<Connection>,
    groups: Vec<Group>,
    identities: Vec<Identity>,
    keys: Vec<SshKey>,
}

impl Vault {
    fn known(&self) -> Known<'_> {
        Known {
            connections: &self.connections,
            groups: &self.groups,
            identities: &self.identities,
            keys: &self.keys,
        }
    }

    fn import(&self, text: &str) -> DirectImport {
        csv_plan::plan(csv::parse(text).expect("table parses"), &self.known())
    }

    fn host(&mut self, label: &str, hostname: &str) -> Connection {
        let conn = Connection::new(label, hostname);
        self.connections.push(conn.clone());
        conn
    }

    fn folder(&mut self, label: &str, parent: Option<&Group>) -> Group {
        let mut g = Group::new(label);
        g.parent_id = parent.map(|p| p.id);
        self.groups.push(g.clone());
        g
    }

    /// The file this vault exports, as `ExportHostsCsv` builds it.
    fn export(&self) -> String {
        let rows: Vec<ExportRow<'_>> = self
            .connections
            .iter()
            .map(|c| ExportRow {
                conn: c,
                group: c.group_id.map(|g| Group::path_of(&self.groups, g)).unwrap_or_default(),
                identity: c
                    .identity_id
                    .and_then(|id| self.identities.iter().find(|i| i.id == id))
                    .map(|i| i.label.clone())
                    .unwrap_or_default(),
                key: c
                    .key_id
                    .and_then(|id| self.keys.iter().find(|k| k.id == id))
                    .map(|k| k.label.clone())
                    .unwrap_or_default(),
            })
            .collect();
        csv::render(&rows)
    }
}

fn changes(op: &HostOp) -> Vec<&'static str> {
    match op {
        HostOp::Update { changes } => changes.clone(),
        other => panic!("not an update: {other:?}"),
    }
}

#[test]
fn header_mapping_is_order_and_style_insensitive() {
    let import = Vault::default().import(
        "Session Name,SSH Username,Host Name,SSH Port\n\
         web,deploy,web.example.com,2222\n",
    );
    assert_eq!(import.hosts.len(), 1);
    let c = &import.hosts[0].conn;
    assert_eq!(c.label, "web");
    assert_eq!(c.hostname, "web.example.com");
    assert_eq!(c.port, 2222);
    assert_eq!(c.username.as_deref(), Some("deploy"));
    assert_eq!(import.hosts[0].op, HostOp::Add);
    assert_eq!(import.hosts[0].folder, FolderPlan::Open);
}

#[test]
fn quotes_semicolons_tags_and_passwords() {
    // European-style semicolons, a quoted field with a comma and an
    // escaped quote, tags, and a password column.
    let import = Vault::default().import(
        "name;host;tags;notes;password\n\
         db;db1.corp;\"prod,db\";\"say \"\"hi\"\", then wait\";hunter2\n",
    );
    let host = &import.hosts[0];
    assert_eq!(host.conn.tags, vec!["prod".to_string(), "db".to_string()]);
    assert_eq!(host.conn.notes.as_deref(), Some("say \"hi\", then wait"));
    assert_eq!(host.password.as_deref(), Some("hunter2"));
    assert_eq!(host.conn.auth_method, AuthMethod::Password);
}

#[test]
fn one_column_of_hostnames_still_works() {
    let import = Vault::default().import("hostname\nweb1.example.com\nweb2.example.com\n");
    assert_eq!(import.hosts.len(), 2);
    // Label falls back to the address.
    assert_eq!(import.hosts[0].conn.label, "web1.example.com");
}

#[test]
fn rows_without_an_address_and_foreign_protocols_are_reported() {
    let import = Vault::default().import(
        "name,host,protocol\n\
         ok,h1,ssh\n\
         ,,\n\
         rdp-box,h2,rdp\n\
         ,,ssh\n",
    );
    assert_eq!(import.hosts.len(), 1);
    // The RDP row is named by its label, the blank-but-present row by
    // its file line, each with the reason.
    assert_eq!(import.skipped.len(), 2);
    assert!(import.skipped[0].starts_with("rdp-box ("), "{:?}", import.skipped);
    assert!(import.skipped[1].starts_with("line 5 ("), "{:?}", import.skipped);
}

#[test]
fn a_port_that_is_not_one_is_refused_not_defaulted() {
    let import = Vault::default().import("name,host,port\na,a.corp,abc\nb,b.corp,0\nc,c.corp,\n");
    assert_eq!(import.hosts.len(), 1);
    assert_eq!(import.hosts[0].conn.port, 22);
    assert_eq!(import.skipped.len(), 2);
}

#[test]
fn a_user_at_host_cell_is_split_like_every_other_importer() {
    let import = Vault::default().import("host\nroot@10.0.0.7:2222\n");
    let c = &import.hosts[0].conn;
    assert_eq!(c.hostname, "10.0.0.7");
    assert_eq!(c.username.as_deref(), Some("root"));
    assert_eq!(c.port, 2222);
}

#[test]
fn a_row_with_an_id_updates_that_host_and_keeps_what_the_file_does_not_name() {
    let mut v = Vault::default();
    let mut known = Connection::new("web", "old.corp");
    known.username = Some("deploy".into());
    known.notes = Some("keep me".into());
    known.agent_forwarding = true;
    v.connections.push(known.clone());

    let import = v.import(&format!("id,host,user\n{},new.corp,\n", known.id));
    assert!(import.skipped.is_empty(), "{:?}", import.skipped);
    let host = &import.hosts[0];
    assert_eq!(host.conn.id, known.id);
    assert_eq!(host.conn.hostname, "new.corp");
    // A column the file has, left empty, clears the field (the folder
    // or identity can supply the login again).
    assert_eq!(host.conn.username, None);
    // Columns the file does not have stay as they were.
    assert_eq!(host.conn.label, "web");
    assert_eq!(host.conn.notes.as_deref(), Some("keep me"));
    assert!(host.conn.agent_forwarding);
    assert_eq!(host.conn.created_at, known.created_at);
    assert_eq!(host.folder, FolderPlan::Keep);
    assert_eq!(changes(&host.op), vec!["hostname", "username"]);
}

#[test]
fn an_unknown_id_adds_with_a_fresh_one() {
    // A file from another vault: its ids name nothing here, and reusing
    // one would merge two hosts the next time the vaults sync.
    let foreign = uuid::Uuid::new_v4();
    let import = Vault::default().import(&format!("id,label,host\n{foreign},web,web.corp\n"));
    let host = &import.hosts[0];
    assert_eq!(host.op, HostOp::Add);
    assert_ne!(host.conn.id, foreign);
}

#[test]
fn an_add_or_update_action_is_the_same_as_none() {
    let mut v = Vault::default();
    let known = v.host("web", "web.corp");
    let import = v.import(&format!(
        "id,action,label,host\n{},Update,web,web2.corp\n,ADD,db,db.corp\n",
        known.id
    ));
    assert!(import.skipped.is_empty(), "{:?}", import.skipped);
    assert!(matches!(import.hosts[0].op, HostOp::Update { .. }));
    assert_eq!(import.hosts[1].op, HostOp::Add);
}

#[test]
fn an_unknown_action_and_a_mistyped_id_do_nothing() {
    let import = Vault::default().import(
        "id,action,label,host\n,archive,a,a.corp\n5b4e0d52-0000-4000-8000-00000000000z,,b,b.corp\n",
    );
    assert!(import.hosts.is_empty());
    assert_eq!(import.skipped.len(), 2, "{:?}", import.skipped);
}

#[test]
fn an_inventory_sheets_own_ids_are_not_vault_ids() {
    // A CMDB export numbers its rows; those carry no instruction here.
    let import = Vault::default().import("id,host,user\n1,a.corp,deploy\nsrv-17,b.corp,\n");
    assert!(import.skipped.is_empty(), "{:?}", import.skipped);
    assert_eq!(import.hosts.len(), 2);
    assert!(import.hosts.iter().all(|h| h.op == HostOp::Add));
}

#[test]
fn a_row_refused_for_its_own_reason_does_not_claim_its_host() {
    let mut v = Vault::default();
    let known = v.host("web", "web.corp");
    let import = v.import(&format!(
        "id,host,identity\n{id},web2.corp,nobody\n{id},web3.corp,\n",
        id = known.id
    ));
    assert_eq!(import.skipped.len(), 1, "{:?}", import.skipped);
    assert_eq!(import.hosts.len(), 1);
    assert_eq!(import.hosts[0].conn.hostname, "web3.corp");
}

#[test]
fn remove_names_a_host_by_id_or_by_a_label_only_one_host_has() {
    let mut v = Vault::default();
    let by_id = v.host("api", "api.corp");
    let by_label = v.host("db", "db.corp");
    v.host("twin", "a.corp");
    v.host("twin", "b.corp");
    v.host("same", "s.corp");
    v.host("same", "s.corp");
    let import = v.import(&format!(
        "id,action,label,host\n\
         {},remove,,\n\
         ,remove,db,\n\
         ,remove,twin,b.corp\n\
         ,remove,same,\n\
         ,remove,ghost,\n\
         {},remove,,\n\
         ,remove,,\n",
        by_id.id,
        uuid::Uuid::new_v4()
    ));
    let removed: Vec<&str> = import
        .hosts
        .iter()
        .filter(|h| h.op == HostOp::Remove)
        .map(|h| h.conn.hostname.as_str())
        .collect();
    // The address tells two hosts of one label apart; two hosts that
    // match on both are refused, never picked.
    assert_eq!(removed, vec!["api.corp", by_label.hostname.as_str(), "b.corp"]);
    // `same` is ambiguous, `ghost` names nobody, the unknown id names
    // nothing, and the last row names no host at all.
    assert_eq!(import.skipped.len(), 4, "{:?}", import.skipped);
}

#[test]
fn a_host_the_file_names_twice_is_refused_the_second_time() {
    let mut v = Vault::default();
    let known = v.host("web", "web.corp");
    let import = v.import(&format!(
        "id,action,host\n{id},,web2.corp\n{id},remove,\n",
        id = known.id
    ));
    assert_eq!(import.hosts.len(), 1);
    assert!(matches!(import.hosts[0].op, HostOp::Update { .. }));
    assert_eq!(import.skipped.len(), 1);
}

#[test]
fn the_folder_column_places_the_host_instead_of_riding_its_notes() {
    let mut v = Vault::default();
    let prod = v.folder("Prod", None);
    let web = v.folder("Web", Some(&prod));
    let import = v.import(
        "name,host,folder\n\
         a,a.corp,Prod / Web\n\
         b,b.corp,Prod/Web\n\
         c,c.corp,Web\n\
         d,d.corp,Prod / Api\n\
         e,e.corp,\n",
    );
    let folders: Vec<&FolderPlan> = import.hosts.iter().map(|h| &h.folder).collect();
    assert_eq!(
        folders,
        vec![
            &FolderPlan::Existing(web.id),
            &FolderPlan::Existing(web.id),
            // A bare name finds the one nested folder carrying it.
            &FolderPlan::Existing(web.id),
            &FolderPlan::Create("Prod / Api".into()),
            // Blank: wherever the import was opened.
            &FolderPlan::Open,
        ]
    );
    assert!(import.hosts.iter().all(|h| h.conn.notes.is_none()));
}

#[test]
fn a_folder_name_two_folders_carry_and_a_dynamic_folder_are_refused() {
    let mut v = Vault::default();
    let prod = v.folder("Prod", None);
    let staging = v.folder("Staging", None);
    v.folder("Web", Some(&prod));
    v.folder("Web", Some(&staging));
    let mut cloud = Group::new("AWS");
    cloud.cloud_query = Some(CloudQuery {
        profile_id: uuid::Uuid::new_v4(),
        kind: CloudQueryKind::EcsTasks {
            cluster: "c".into(),
            service: "s".into(),
            container: String::new(),
        },
        template: ConnectionTemplate::new(TransportKind::EcsExec),
    });
    v.groups.push(cloud);
    let import = v.import("name,host,folder\na,a.corp,Web\nb,b.corp,AWS\nc,c.corp,AWS / Sub\n");
    assert!(import.hosts.is_empty());
    assert_eq!(import.skipped.len(), 3, "{:?}", import.skipped);
}

#[test]
fn an_update_with_a_blank_folder_goes_to_the_top_and_without_the_column_stays() {
    let mut v = Vault::default();
    let prod = v.folder("Prod", None);
    let mut known = Connection::new("web", "web.corp");
    known.group_id = Some(prod.id);
    v.connections.push(known.clone());
    let blank = v.import(&format!("id,folder\n{},\n", known.id));
    assert_eq!(blank.hosts[0].folder, FolderPlan::Root);
    assert_eq!(changes(&blank.hosts[0].op), vec!["group"]);
    let absent = v.import(&format!("id,action\n{},\n", known.id));
    assert_eq!(absent.hosts[0].folder, FolderPlan::Keep);
    assert!(changes(&absent.hosts[0].op).is_empty());
}

#[test]
fn identity_and_key_are_named_by_exact_label() {
    let mut v = Vault::default();
    let ops = Identity::new("ops");
    v.identities.push(ops.clone());
    v.identities.push(Identity::new("shared"));
    v.identities.push(Identity::new("shared"));
    let key = SshKey::new("deploy-key", KeyAlgorithm::Ed25519);
    v.keys.push(key.clone());
    let import = v.import(
        "name,host,identity,key\n\
         a,a.corp,ops,deploy-key\n\
         b,b.corp,shared,\n\
         c,c.corp,nobody,\n\
         d,d.corp,,no-key\n",
    );
    assert_eq!(import.hosts.len(), 1);
    assert_eq!(import.hosts[0].conn.identity_id, Some(ops.id));
    assert_eq!(import.hosts[0].conn.key_id, Some(key.id));
    assert_eq!(import.skipped.len(), 3, "{:?}", import.skipped);
}

#[test]
fn an_update_keeps_an_identity_whose_label_another_shares() {
    // The export writes the label of the identity the host HAS; read
    // back, that label means that identity even though it is not
    // unique, so a round trip never refuses its own file.
    let mut v = Vault::default();
    let mine = Identity::new("shared");
    v.identities.push(mine.clone());
    v.identities.push(Identity::new("shared"));
    let mut known = Connection::new("web", "web.corp");
    known.identity_id = Some(mine.id);
    v.connections.push(known.clone());
    let import = v.import(&format!("id,identity\n{},shared\n", known.id));
    assert!(import.skipped.is_empty(), "{:?}", import.skipped);
    assert_eq!(import.hosts[0].conn.identity_id, Some(mine.id));
}

#[test]
fn an_update_may_not_turn_a_host_into_a_protocol_an_add_cannot_create() {
    let mut v = Vault::default();
    let mut rdp = Connection::new("desk", "desk.corp");
    rdp.protocol = ConnectionProtocol::RemoteDesktop;
    v.connections.push(rdp.clone());
    let ssh = v.host("web", "web.corp");
    // The RDP host's own token keeps it; telnet is a protocol an add
    // creates, rdp is not.
    let import = v.import(&format!(
        "id,protocol\n{},rdp\n{},telnet\n",
        rdp.id, ssh.id
    ));
    assert!(import.skipped.is_empty(), "{:?}", import.skipped);
    assert_eq!(import.hosts[0].conn.protocol, ConnectionProtocol::RemoteDesktop);
    assert_eq!(import.hosts[1].conn.protocol, ConnectionProtocol::Telnet);
    let refused = v.import(&format!("id,protocol\n{},rdp\n", ssh.id));
    assert!(refused.hosts.is_empty());
}

#[test]
fn a_blank_password_cell_never_clears_the_stored_one() {
    let mut v = Vault::default();
    let known = v.host("web", "web.corp");
    let import = v.import(&format!("id,password\n{},\n", known.id));
    assert_eq!(import.hosts[0].password, None);
    assert!(changes(&import.hosts[0].op).is_empty());
    let typed = v.import(&format!("id,password\n{},s3cret\n", known.id));
    assert_eq!(typed.hosts[0].password.as_deref(), Some("s3cret"));
    assert_eq!(changes(&typed.hosts[0].op), vec!["password"]);
}

#[test]
fn a_cloud_host_remembers_what_the_import_changed() {
    let mut v = Vault::default();
    let mut known = Connection::new("i-0abc", "10.0.0.7");
    known.cloud_ref = Some(CloudRef {
        profile_id: uuid::Uuid::new_v4(),
        resource_type: CloudResourceType::Ec2,
        resource_id: "i-0abc".into(),
        region: None,
        transport_pref: TransportKind::EcsExec,
        auto_refresh_hostname: false,
        orphaned_at: None,
    });
    v.connections.push(known.clone());
    let import = v.import(&format!("id,label\n{},web\n", known.id));
    assert_eq!(import.hosts[0].conn.customized_fields, vec!["label".to_string()]);
}

#[test]
fn an_export_reads_back_as_a_file_that_changes_nothing() {
    let mut v = Vault::default();
    let prod = v.folder("Prod", None);
    let web = v.folder("Web", Some(&prod));
    let ops = Identity::new("ops");
    v.identities.push(ops.clone());
    let key = SshKey::new("deploy-key", KeyAlgorithm::Ed25519);
    v.keys.push(key.clone());
    let mut a = Connection::new("web, staging", "web1.corp");
    a.port = 2222;
    a.username = Some("deploy".into());
    a.tags = vec!["prod".into(), "web".into()];
    a.notes = Some("say \"hi\"\nthen wait".into());
    a.group_id = Some(web.id);
    a.identity_id = Some(ops.id);
    a.key_id = Some(key.id);
    v.connections.push(a);
    let mut b = Connection::new("serial", "/dev/ttyUSB0");
    b.protocol = ConnectionProtocol::Serial;
    v.connections.push(b);
    v.host("plain", "plain.corp");

    let import = v.import(&v.export());
    assert!(import.skipped.is_empty(), "{:?}", import.skipped);
    assert_eq!(import.hosts.len(), 3);
    for host in &import.hosts {
        assert_eq!(changes(&host.op), Vec::<&str>::new(), "{}", host.conn.label);
        let before = v.connections.iter().find(|c| c.id == host.conn.id).unwrap();
        // Field by field: nothing the round trip touched may differ.
        assert_eq!(
            serde_json::to_value(&host.conn).unwrap(),
            serde_json::to_value(before).unwrap(),
            "{}",
            host.conn.label
        );
    }
}

#[test]
fn an_export_read_into_an_empty_vault_rebuilds_its_folders() {
    let mut v = Vault::default();
    let prod = v.folder("Prod", None);
    let web = v.folder("Web", Some(&prod));
    let mut a = Connection::new("a", "a.corp");
    a.group_id = Some(web.id);
    v.connections.push(a);
    let mut b = Connection::new("b", "b.corp");
    b.group_id = Some(prod.id);
    v.connections.push(b);

    let empty = Vault::default();
    let import = empty.import(&v.export());
    assert!(import.hosts.iter().all(|h| h.op == HostOp::Add));
    let ticked = vec![true; import.hosts.len()];
    assert_eq!(
        csv_plan::folders_to_create(&empty.groups, &import.hosts, &ticked),
        vec!["Prod".to_string(), "Prod / Web".to_string()]
    );
    // An unticked row creates nothing.
    assert_eq!(
        csv_plan::folders_to_create(&empty.groups, &import.hosts, &[false, true]),
        vec!["Prod".to_string()]
    );
}

#[test]
fn rows_from_other_importers_pass_through_untouched() {
    let mut import = DirectImport {
        source_key: "import_putty_btn",
        hosts: vec![super::DirectHost::new(Connection::new("p", "p.corp"), None)],
        skipped: Vec::new(),
    };
    import = csv_plan::plan(import, &Vault::default().known());
    assert_eq!(import.hosts.len(), 1);
    assert_eq!(import.hosts[0].op, HostOp::Add);
}
