use fork::{daemon, Fork};
use rustyline::{
    error::ReadlineError,
    // validate::{ValidationContext, ValidationResult, Validator},
    Editor,
    Result as RustyResult,
};
use rustyline_derive::{Completer, Helper, Highlighter, Hinter};
use std::env;
use std::os::unix::fs::FileTypeExt;
use std::process::{self, Command};
use std::{fs, io};
use zellij_utils::{
    consts::ZELLIJ_SOCK_DIR,
    envs,
    interprocess::local_socket::LocalSocketStream,
    ipc::{ClientToServerMsg, IpcReceiverWithContext, IpcSenderWithContext, ServerToClientMsg},
};

fn main() {
    // It seems helpful to protect the user from spawning a nested Zellij session
    let _ = env::vars_os().into_iter().map(|v| {
        if v.0.into_string().unwrap().contains("ZELLIJ") {
            std::process::exit(-1);
        }
    });

    // ToDo
    // Check if the client supplied an argv parameter for the session name they want
    let session: Option<String> = env::args().nth(1_usize);
    let running_sessions = match get_sessions() {
        Err(err) if io::ErrorKind::NotFound != err => exit_zellij_not_found(),
        Err(_) => Vec::<String>::new(),
        Ok(sessions) => sessions,
    };

    match session.clone() {
        None => {
            let _ = interactive_select(&running_sessions);
        }
        Some(session_name) => match try_joining(&session_name, &running_sessions) {
            Ok(_) => (),
            Err(_) => {
                spawn(session_name).expect("This should be infallible");
            }
        },
    };
    connect(session.unwrap());
    // At this point, we should have checked against (1) broken zellij installations,
    // (2) a session name passed from STDIN, where we would have joined
}

fn exit_zellij_not_found() -> ! {
    println!("Looks like zellij isn't available. Exiting.");
    std::process::exit(-1);
}

fn try_joining<T>(session_name: T::Item, sessions: T) -> io::Result<()>
where
    T: IntoIterator,
    T::Item: AsRef<str>,
{
    match sessions
        .into_iter()
        .find(|s| s.as_ref() == session_name.as_ref())
    {
        None => {
            // We didn't find the session name matching the one requested
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Could not find session {}",
            ))
        }
        Some(_) => Ok(()),
    }
}

// Retrieved from Zellij
// https://github.com/zellij-org/zellij/blob/main/src/sessions.rs
fn get_sessions() -> Result<Vec<String>, io::ErrorKind> {
    match fs::read_dir(&*zellij_utils::consts::ZELLIJ_SOCK_DIR) {
        Ok(files) => {
            let mut sessions = Vec::new();
            files.for_each(|file| {
                let file = file.unwrap();
                let file_name = file.file_name().into_string().unwrap();
                if file.file_type().unwrap().is_socket() && assert_socket(&file_name) {
                    sessions.push(file_name);
                }
            });
            Ok(sessions)
        }
        Err(err) if io::ErrorKind::NotFound != err.kind() => Err(err.kind()),
        Err(_) => Ok(Vec::with_capacity(0)),
    }
}

fn assert_socket(name: &str) -> bool {
    let path = &*ZELLIJ_SOCK_DIR.join(name);
    match LocalSocketStream::connect(path) {
        Ok(stream) => {
            let mut sender = IpcSenderWithContext::new(stream);
            let _ = sender.send(ClientToServerMsg::ConnStatus);
            let mut receiver: IpcReceiverWithContext<ServerToClientMsg> = sender.get_receiver();
            match receiver.recv() {
                Some((ServerToClientMsg::Connected, _)) => true,
                None | Some((_, _)) => false,
            }
        }
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            drop(fs::remove_file(path));
            false
        }
        Err(_) => false,
    }
}

fn spawn<T: Into<String>>(session: T) -> io::Result<()> {
    Ok(())
}

fn connect<T: AsRef<std::ffi::OsStr>>(session: T) -> Result<std::process::Child, std::io::Error> {
    // The tricky part here is that we don't want to occupy
    // two entire processes, where one of them is a deadbeat parent
    // So, my idea here is to fork into a daemon, but preserve all the
    // relevant pipes
    if let Ok(Fork::Child) = daemon(
        /* nochdir: bool = */ false, /* noclose: bool = */ true,
    ) {
        // Opting to use `.spawn()` since it inherits the pipes
        // Otherwise, `.output()` would create new ones and detach
        Command::new("zellij").arg("-a").arg(session).spawn()
    } else {
        Err(std::io::Error::new(
            io::ErrorKind::BrokenPipe,
            "Broke the forked connection",
        ))
    }
}

fn interactive_select<T>(sessions: T) -> Result<(), Box<dyn std::error::Error>>
where
    T: IntoIterator,
    T::Item: AsRef<str> + std::fmt::Display,
{
    println!("Create a new session by entering the name for it, or select one from these options:");

    let mut repl = Editor::<()>::new()?;

    ctrlc::set_handler(move || {
        println!("\rEnter nil to drop to normal prompt");
    })
    .expect("Error setting Ctrl-C handler");

    let stdin: String = loop {
        for (id, session) in sessions.into_iter().enumerate() {
            println!("({}) :: {}", &stringify!(id), &session);
        }
        let feed = repl.readline(">>> ")?.as_str();
        if feed.is_empty() {
            continue;
        }
        if let Some(_) = &feed.find(char::is_whitespace) {
            continue;
        }
        break feed.to_string();
    };
    spawn(&stdin)?;

    Ok(())
}
