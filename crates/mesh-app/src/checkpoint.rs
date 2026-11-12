//! PLAN-dm Checkpoint B, steps 1 to 6, by machine: three copies of the app,
//! each with a real `mesh-node`, a loopback relay and a seed, driven with the
//! messages Mesh's buttons send, and checked through the app's own state (the
//! chat list the sidebar draws, unread counts, timeline rows and their
//! marks). Nothing is drawn.
//!
//! It needs the Mesh-Node checkout beside this one (as `installer/build.ps1`
//! does), or `MESH_NODE` naming it; it builds `mesh-node` and the
//! `test_relay` example there. `cargo test -p mesh-app checkpoint -- --ignored`

use super::*;
use crate::mesh_dm::Click;
use crate::mesh_ui::Msg;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PASSWORD: &str = "correct horse battery staple";
/// The longest any one step waits.
const STEP: Duration = Duration::from_secs(90);

/// The Mesh-Node checkout's built binaries: `mesh-node` and the test relay.
fn mesh_node_bins() -> (PathBuf, PathBuf) {
    let dir = std::env::var_os("MESH_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../Mesh-Node"));
    assert!(dir.join("Cargo.toml").is_file(), "no Mesh-Node checkout at {} (set MESH_NODE)", dir.display());
    let built = Command::new("cargo")
        .args(["build", "-q", "-p", "mesh-node", "--bin", "mesh-node", "--example", "test_relay"])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(built.success(), "mesh-node didn't build");
    (dir.join("target/debug/mesh-node"), dir.join("target/debug/examples/test_relay"))
}

/// A child process that's killed when the test ends, however it ends.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One copy of the app: its state, the helper's events, and the task that
/// runs the helper (aborting it closes the copy, as quitting Mesh does).
struct Copy {
    app: App,
    events: tokio::sync::mpsc::UnboundedReceiver<mesh::Event>,
    helper: tokio::task::JoinHandle<()>,
    name: &'static str,
    id: String,
}

struct World {
    rt: tokio::runtime::Runtime,
    exe: PathBuf,
    args: Vec<String>,
    root: PathBuf,
}

impl World {
    /// A copy's app on its own data, its helper started (Mesh opening).
    fn open(&self, name: &'static str) -> Copy {
        let (mut app, _) = boot();
        app.mesh_node = Some(self.exe.clone());
        app.sounds.enabled = false;
        app.window_focused = true;
        let (tx, events) = tokio::sync::mpsc::unbounded_channel();
        let helper = self.rt.spawn(mesh::run_with(self.exe.clone(), self.args.clone(), self.root.join(name), tx));
        Copy {
            app,
            events,
            helper,
            name,
            id: String::new(),
        }
    }

    /// Takes the helper's events into the app until `done` holds.
    fn until(&self, copy: &mut Copy, what: &str, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + STEP;
        while !done(&copy.app) {
            assert!(Instant::now() < deadline, "{}: {what} never came", copy.name);
            let next = self
                .rt
                .block_on(async { tokio::time::timeout(Duration::from_millis(200), copy.events.recv()).await });
            if let Ok(Some(event)) = next {
                let _ = update(&mut copy.app, Message::Mesh(event));
            }
        }
    }

    /// Keeps sending `msg` (a button pressed again, as a person would) until
    /// `done` holds: the forms wait on the helper's answers.
    fn press_until(&self, copy: &mut Copy, what: &str, msg: impl Fn() -> Message, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + STEP;
        while !done(&copy.app) {
            assert!(Instant::now() < deadline, "{}: {what} never came", copy.name);
            let _ = update(&mut copy.app, msg());
            let pause = Instant::now() + Duration::from_millis(500);
            while Instant::now() < pause && !done(&copy.app) {
                let next = self
                    .rt
                    .block_on(async { tokio::time::timeout(Duration::from_millis(100), copy.events.recv()).await });
                if let Ok(Some(event)) = next {
                    let _ = update(&mut copy.app, Message::Mesh(event));
                }
            }
        }
    }

    /// Onboarding: a username and a password, then Continue and Create.
    fn create(&self, copy: &mut Copy) {
        self.until(copy, "the helper", |a| a.mesh.node.is_some() && a.mesh.identity.is_some());
        for msg in [Msg::Username(copy.name.into()), Msg::Password(PASSWORD.into()), Msg::Confirm(PASSWORD.into())] {
            let _ = update(&mut copy.app, Message::MeshForm(msg));
        }
        self.press_until(copy, "the identity", || Message::MeshForm(Msg::Submit), unlocked);
        copy.id = copy.app.mesh.my_id().unwrap();
    }

    /// Unlocking a copy that was closed.
    fn unlock(&self, copy: &mut Copy) {
        self.until(copy, "the helper", |a| a.mesh.node.is_some() && a.mesh.identity.is_some());
        for msg in [Msg::Username(copy.name.into()), Msg::Password(PASSWORD.into())] {
            let _ = update(&mut copy.app, Message::MeshForm(msg));
        }
        self.press_until(copy, "unlocking", || Message::MeshForm(Msg::Submit), unlocked);
    }

    /// Check an ID, then Add by ID.
    fn add(&self, copy: &mut Copy, other: &str) {
        let _ = update(&mut copy.app, Message::MeshForm(Msg::CheckInput(other.into())));
        let _ = update(&mut copy.app, Message::MeshForm(Msg::Check));
        let known = |a: &App| (a.mesh.social.outgoing.iter()).chain(&a.mesh.social.blocked).any(|k| k.id == other) || is_friend(a, other);
        self.press_until(copy, "the add", || Message::MeshForm(Msg::AddById), known);
    }
}

fn unlocked(app: &App) -> bool {
    app.mesh.identity.as_ref().is_some_and(|i| i.state == mesh::State::Unlocked) && matches!(app.screen, Screen::LoggedIn(_))
}

fn is_friend(app: &App, id: &str) -> bool {
    app.mesh.social.friends.iter().any(|f| f.id == id)
}

fn session(app: &App) -> &Session {
    match &app.screen {
        Screen::LoggedIn(session) => session,
        _ => panic!("not in the app yet"),
    }
}

/// The chat list's row for `id` (what the sidebar draws).
fn chat<'a>(app: &'a App, id: &str) -> Option<&'a mesh::Chat> {
    app.mesh.chats.iter().find(|c| c.id == id)
}

/// The open chat's rows, as (body, mark).
fn rows(app: &App) -> Vec<(String, String)> {
    let s = session(app);
    (s.timeline.iter())
        .map(|m| {
            let mark = m.web_id.as_ref().and_then(|id| s.web_marks.get(id)).cloned().unwrap_or_default();
            (m.body.clone(), mark)
        })
        .collect()
}

fn has_row(app: &App, body: &str) -> bool {
    rows(app).iter().any(|(b, _)| b == body)
}

fn row<'a>(app: &'a App, body: &str) -> &'a TimelineMessage {
    session(app)
        .timeline
        .iter()
        .find(|m| m.body == body)
        .unwrap_or_else(|| panic!("no row {body:?}"))
}

fn mark(app: &App, body: &str) -> String {
    rows(app).into_iter().find(|(b, _)| b == body).map(|(_, m)| m).unwrap_or_default()
}

/// Opens a chat, as clicking its sidebar row (or Message on a friend row) does.
fn open(copy: &mut Copy, id: &str, dm: bool) {
    let name = copy.app.mesh.name_of(id);
    let _ = update(
        &mut copy.app,
        Message::WebChatSelected(WebChat {
            network: Network::Mesh,
            id: id.into(),
            name,
            dm,
        }),
    );
}

/// Types `text` into the composer, then Enter.
fn send(copy: &mut Copy, text: &str) {
    for action in [
        text_editor::Action::SelectAll,
        text_editor::Action::Edit(text_editor::Edit::Paste(Arc::new(text.to_owned()))),
    ] {
        let _ = update(&mut copy.app, Message::ComposerEdit(action));
    }
    let _ = update(&mut copy.app, Message::SendMessage);
}

/// The paperclip with a file picked, then Enter.
fn send_file(copy: &mut Copy, path: &Path) {
    let filename = path.file_name().unwrap().to_string_lossy().into_owned();
    let attachment = Attachment {
        mime: mime_guess_from(&filename),
        filename,
        data: std::fs::read(path).unwrap(),
        path: Some(path.to_owned()),
    };
    let _ = update(&mut copy.app, Message::AttachmentPicked(Some(attachment)));
    let _ = update(&mut copy.app, Message::SendMessage);
}

#[test]
#[ignore = "needs the Mesh-Node checkout beside this one: cargo test -p mesh-app checkpoint -- --ignored"]
fn checkpoint_b_steps_1_to_6() {
    let (exe, relay_exe) = mesh_node_bins();
    let root = std::env::temp_dir().join(format!("mesh-checkpoint-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // boot() reads its settings from here; each copy's helper gets its own folder.
    unsafe { std::env::set_var("MESH_DATA", &root) };

    let mut relay = Command::new(&relay_exe).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut url = String::new();
    BufReader::new(relay.stdout.take().unwrap()).read_line(&mut url).unwrap();
    let url = url.trim().to_owned();
    let _relay = Running(relay);
    let mut seed = Command::new(&exe)
        .args(["seed", "--data"])
        .arg(root.join("seed"))
        .args(["--relay", &url])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let ticket = (BufReader::new(seed.stderr.take().unwrap()).lines().map_while(Result::ok))
        .find_map(|l| l.strip_prefix("mesh-node seed: ticket ").map(str::to_owned))
        .expect("the seed's ticket");
    let _seed = Running(seed);
    let world = World {
        rt: tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap(),
        exe,
        args: ["--relay", &url, "--seed", &ticket, "--record-ttl", "8"].map(String::from).to_vec(),
        root: root.clone(),
    };

    // 1. Friends: three identities, each adds the other two by ID.
    let (mut a, mut b, mut c) = (world.open("alice"), world.open("bob"), world.open("carol"));
    for copy in [&mut a, &mut b, &mut c] {
        world.create(copy);
    }
    let ids = [a.id.clone(), b.id.clone(), c.id.clone()];
    for copy in [&mut a, &mut b, &mut c] {
        let me = copy.id.clone();
        for other in ids.iter().filter(|id| **id != me) {
            world.add(copy, other);
        }
    }
    for copy in [&mut a, &mut b, &mut c] {
        let others: Vec<String> = ids.iter().filter(|id| **id != copy.id).cloned().collect();
        world.until(copy, "both friends", |app| others.iter().all(|o| is_friend(app, o)));
    }
    let [id_a, id_b, id_c] = ids;

    // 2. Message on B's friend row; send. It shows on B with an unread count
    // that opening clears, and A's "sending" mark goes once B has it.
    open(&mut a, &id_b, true);
    send(&mut a, "one");
    world.until(&mut a, "A's own row", |app| has_row(app, "one"));
    world.until(&mut b, "A's chat in B's list, unread", |app| {
        chat(app, &id_a).is_some_and(|c| c.unread == 1 && c.kind == mesh::ChatKind::Direct)
    });
    world.until(&mut a, "\"sent\"", |app| has_row(app, "one") && mark(app, "one").is_empty());
    open(&mut b, &id_a, true);
    world.until(&mut b, "the chat open on B", |app| has_row(app, "one"));
    world.until(&mut b, "unread cleared", |app| chat(app, &id_a).is_some_and(|c| c.unread == 0));

    // Reply, react, edit, delete for everyone.
    let one = row(&b.app, "one").web_id.clone().unwrap();
    let quote = mesh_core::Quote {
        author: "alice".into(),
        text: "one".into(),
    };
    let _ = update(&mut b.app, Message::ReplyTo(one.clone(), quote));
    send(&mut b, "two");
    world.until(&mut a, "B's reply on A", |app| has_row(app, "two"));
    assert_eq!(row(&a.app, "two").reply_to.as_ref().map(|q| q.text.as_str()), Some("one"), "it quotes one");
    let two = row(&a.app, "two").web_id.clone().unwrap();
    let _ = update(
        &mut a.app,
        Message::React {
            id: two.clone(),
            emoji: "👍".into(),
        },
    );
    world.until(&mut b, "A's reaction on B", |app| {
        has_row(app, "two") && row(app, "two").reactions.iter().any(|r| r.key == "👍" && !r.by_me)
    });
    // Esc leaves an edit, as it leaves a reply.
    let _ = update(&mut b.app, Message::MeshEdit(two.clone(), "two".into()));
    assert!(session(&b.app).editing.is_some());
    let _ = update(&mut b.app, Message::Escape);
    assert!(session(&b.app).editing.is_none() && draft(session(&b.app)).is_empty(), "Esc leaves the edit");
    let _ = update(&mut b.app, Message::MeshEdit(two.clone(), "two".into()));
    send(&mut b, "two, edited");
    world.until(&mut a, "the edit on A", |app| {
        has_row(app, "two, edited") && mark(app, "two, edited") == "edited"
    });
    world.until(&mut b, "the edit on B", |app| {
        has_row(app, "two, edited") && mark(app, "two, edited").contains("edited")
    });
    let _ = update(&mut a.app, Message::MeshDelete(one.clone(), true));
    world.until(&mut b, "the delete on B", |app| !has_row(app, "one") && has_row(app, "two, edited"));
    world.until(&mut a, "the delete on A", |app| !has_row(app, "one"));

    // 3. A picture: its preview at once on B, the full one when opened.
    let picture = root.join("map.png");
    let pixels = image::RgbImage::from_fn(64, 48, |x, y| image::Rgb([(x * 4) as u8, (y * 5) as u8, 128]));
    image::DynamicImage::ImageRgb8(pixels).save(&picture).unwrap();
    send_file(&mut a, &picture);
    world.until(&mut b, "the picture's row", |app| has_row(app, "map.png"));
    let MessageKind::Image(info) = row(&b.app, "map.png").kind.clone() else {
        panic!("not a picture: {:?}", row(&b.app, "map.png").kind)
    };
    assert!(
        matches!(session(&b.app).images.get(&info.media.key()), Some(Some(_))),
        "the preview shows without a click"
    );
    let full = info.full.as_ref().map(MediaRef::key).unwrap();
    let _ = update(&mut b.app, Message::OpenImage(info.media.key(), Some(full.clone())));
    world.until(&mut b, "the full picture", |app| matches!(session(app).images.get(&full), Some(Some(_))));
    assert!(!cache::file(&full).is_some_and(|p| p.exists()), "never in the media cache");

    // 4. run.sh is shown in its folder; a PDF opens.
    let script = root.join("run.sh");
    std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    let pdf = root.join("trail.pdf");
    std::fs::write(&pdf, "%PDF-1.4\n%%EOF\n").unwrap();
    for (path, click) in [(&script, Click::ShowInFolder), (&pdf, Click::Open)] {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        send_file(&mut a, path);
        world.until(&mut b, "the file's row", |app| has_row(app, &name));
        let MessageKind::File(file) = row(&b.app, &name).kind.clone() else {
            panic!("not a file")
        };
        let _ = update(&mut b.app, Message::OpenMedia(file.media.web_url().unwrap().to_owned()));
        world.until(&mut b, "the click", |app| app.mesh.last_click.as_ref().is_some_and(|(n, _)| *n == name));
        assert_eq!(b.app.mesh.last_click.as_ref().map(|(_, c)| *c), Some(click), "{name}");
    }

    // 5. New group of three: in all three lists, messages both ways.
    for msg in [
        Msg::NewGroup(true),
        Msg::GroupPick(id_b.clone()),
        Msg::GroupPick(id_c.clone()),
        Msg::GroupName("trail crew".into()),
        Msg::GroupCreate,
    ] {
        let _ = update(&mut a.app, Message::MeshForm(msg));
    }
    let group = |app: &App| {
        app.mesh
            .chats
            .iter()
            .find(|c| c.kind == mesh::ChatKind::Group && c.name == "trail crew")
            .cloned()
    };
    for copy in [&mut a, &mut b, &mut c] {
        world.until(copy, "the group with three", |app| group(app).is_some_and(|g| g.members.len() == 3));
    }
    let g = group(&a.app).unwrap().id;
    open(&mut a, &g, false);
    send(&mut a, "g1");
    world.until(&mut c, "g1 unread on C", |app| group(app).is_some_and(|g| g.unread == 1));
    open(&mut b, &g, false);
    world.until(&mut b, "g1 on B", |app| has_row(app, "g1"));
    send(&mut b, "g2 from bob");
    world.until(&mut a, "B's line on A", |app| has_row(app, "g2 from bob"));

    // 6. C is closed: two lines in the group and one to C alone wait, then
    // arrive in order when C opens again; A's "sending" clears then.
    c.helper.abort();
    drop(c);
    send(&mut a, "while away 1");
    send(&mut a, "while away 2");
    world.until(&mut b, "the lines on B", |app| has_row(app, "while away 2"));
    open(&mut a, &id_c, true);
    send(&mut a, "c1");
    world.until(&mut a, "c1, sending", |app| has_row(app, "c1"));
    std::thread::sleep(Duration::from_secs(3));
    for _ in 0..20 {
        if let Ok(Some(event)) = world
            .rt
            .block_on(async { tokio::time::timeout(Duration::from_millis(100), a.events.recv()).await })
        {
            let _ = update(&mut a.app, Message::Mesh(event));
        }
    }
    assert_eq!(mark(&a.app, "c1"), "sending", "C is away");
    let mut c = world.open("carol");
    c.id = id_c.clone();
    world.unlock(&mut c);
    world.until(&mut c, "A's 1:1 and the group, unread", |app| {
        chat(app, &id_a).is_some_and(|ch| ch.unread == 1) && group(app).is_some_and(|g| g.unread == 4)
    });
    open(&mut c, &g, false);
    world.until(&mut c, "the group's lines", |app| has_row(app, "while away 2"));
    let lines: Vec<String> = rows(&c.app).into_iter().map(|(body, _)| body).collect();
    assert_eq!(lines, ["g1", "g2 from bob", "while away 1", "while away 2"], "in order");
    world.until(&mut a, "c1 sent", |app| mark(app, "c1").is_empty());

    for copy in [a, b, c] {
        copy.helper.abort();
    }
    drop(world);
    let _ = std::fs::remove_dir_all(root);
}
