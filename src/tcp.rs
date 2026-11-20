//! TCP API: a newline-delimited text protocol.
//!
//! ```text
//! PING                      -> PONG                (no auth; usable as an LB health check)
//! AUTH <token> [org]        -> OK <user> <org_id> | ERR ...
//! GET <key>                 -> VALUE <value> | NIL
//! SET <key> <value>         -> OK
//! SETEX <key> <ttl> <value> -> OK
//! DEL <key>                 -> OK | NIL
//! QUIT                      -> BYE
//! ```
//! Values are the rest of the line (spaces allowed, no newlines). Binary values
//! stored over HTTP that aren't single-line UTF-8 return `ERR binary value; use HTTP`.
//! The session is bound to the token's `exp`; once it passes, send AUTH again.
//! Every key lives in the session's organization. `[org]` (id or alias) is only
//! required when the token's user belongs to more than one organization.

use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::{
    codec::{Framed, LinesCodec, LinesCodecError},
    sync::CancellationToken,
};

use crate::{
    auth::AuthError,
    identity::{Identity, OrgError},
    store::valid_key,
    AppState,
};

pub async fn serve(listener: TcpListener, state: AppState, stop: CancellationToken) {
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            res = listener.accept() => match res {
                Ok((sock, peer)) => {
                    let st = state.clone();
                    let stop = stop.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle(sock, st, stop).await {
                            tracing::debug!(%peer, error = %e, "tcp connection ended with error");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "tcp accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
}

enum Reply {
    Line(String),
    Close(String),
}

async fn handle(sock: TcpStream, st: AppState, stop: CancellationToken) -> anyhow::Result<()> {
    sock.set_nodelay(true)?;
    // Room for the largest value plus command, key and slack. Tokens are ~1–3 KB.
    let max_line = st.max_value_bytes + crate::store::MAX_KEY_LEN + 64;
    let mut io = Framed::new(sock, LinesCodec::new_with_max_length(max_line));
    let mut session: Option<Session> = None;

    loop {
        let line = tokio::select! {
            _ = stop.cancelled() => {
                let _ = io.send("BYE shutting down").await;
                break;
            }
            next = tokio::time::timeout(st.tcp_idle_timeout, io.next()) => match next {
                Err(_) => { let _ = io.send("BYE idle timeout").await; break; }
                Ok(None) => break,
                Ok(Some(Err(LinesCodecError::MaxLineLengthExceeded))) => {
                    let _ = io.send("ERR line too long").await;
                    break;
                }
                Ok(Some(Err(e))) => return Err(e.into()),
                Ok(Some(Ok(l))) => l,
            }
        };

        match dispatch(line.trim_end_matches('\r'), &mut session, &st).await {
            Reply::Line(s) => io.send(s).await?,
            Reply::Close(s) => {
                io.send(s).await?;
                break;
            }
        }
    }
    Ok(())
}

struct Session {
    id: Identity,
    org: String,
}

async fn dispatch(line: &str, session: &mut Option<Session>, st: &AppState) -> Reply {
    let (cmd, rest) = match line.split_once(' ') {
        Some((c, r)) => (c, r),
        None => (line, ""),
    };
    let cmd = cmd.to_ascii_uppercase();
    let ok = |s: &str| Reply::Line(s.to_string());

    match cmd.as_str() {
        "" => return ok("ERR empty command"),
        "PING" => return ok("PONG"),
        "QUIT" => return Reply::Close("BYE".into()),
        "AUTH" => {
            let mut args = rest.split_whitespace();
            let (token, org_arg) = (args.next().unwrap_or(""), args.next());
            return match st.verifier.verify(token).await {
                Ok(id) => match id.org(org_arg) {
                    Ok(org) => {
                        let org = org.id.clone();
                        let reply = format!("OK {} {}", id.principal, org);
                        *session = Some(Session { id, org });
                        Reply::Line(reply)
                    }
                    Err(e) => {
                        *session = None;
                        Reply::Line(match e {
                            OrgError::Ambiguous => format!("ERR {e}: AUTH <token> <org>"),
                            _ => format!("ERR {e}"),
                        })
                    }
                },
                Err(AuthError::Jwks(e)) => {
                    tracing::warn!(error = %e, "auth backend unavailable");
                    ok("ERR auth unavailable")
                }
                Err(e) => {
                    tracing::debug!(reason = %e, "tcp auth rejected");
                    *session = None;
                    ok("ERR unauthorized")
                }
            };
        }
        _ => {}
    }

    // Everything below needs a live session.
    let Some(sess) = session.as_ref() else {
        return ok("ERR not authenticated; send AUTH <token>");
    };
    if sess.id.expired() {
        *session = None;
        return ok("ERR token expired; send AUTH <token>");
    }
    let perms = sess.id.perms;
    let org = sess.org.as_str();

    match cmd.as_str() {
        "GET" => {
            if !perms.read {
                return ok("ERR forbidden");
            }
            let key = rest.trim();
            if !valid_key(key) {
                return ok("ERR invalid key");
            }
            match st.store.get(org, key) {
                None => ok("NIL"),
                Some(v) => match std::str::from_utf8(&v) {
                    Ok(s) if !s.contains('\n') && !s.contains('\r') => {
                        Reply::Line(format!("VALUE {s}"))
                    }
                    _ => ok("ERR binary value; use HTTP"),
                },
            }
        }
        "SET" => {
            if !perms.write {
                return ok("ERR forbidden");
            }
            let Some((key, value)) = rest.split_once(' ') else {
                return ok("ERR usage: SET <key> <value>");
            };
            if !valid_key(key) {
                return ok("ERR invalid key");
            }
            st.store
                .set(org, key, Bytes::copy_from_slice(value.as_bytes()), None);
            ok("OK")
        }
        "SETEX" => {
            if !perms.write {
                return ok("ERR forbidden");
            }
            let mut parts = rest.splitn(3, ' ');
            let (Some(key), Some(ttl), Some(value)) = (parts.next(), parts.next(), parts.next())
            else {
                return ok("ERR usage: SETEX <key> <ttl_seconds> <value>");
            };
            if !valid_key(key) {
                return ok("ERR invalid key");
            }
            let ttl = match ttl.parse::<u64>() {
                Ok(t) if t > 0 => t,
                _ => return ok("ERR ttl must be a positive integer"),
            };
            st.store.set(
                org,
                key,
                Bytes::copy_from_slice(value.as_bytes()),
                Some(Duration::from_secs(ttl)),
            );
            ok("OK")
        }
        "DEL" => {
            if !perms.write {
                return ok("ERR forbidden");
            }
            let key = rest.trim();
            if !valid_key(key) {
                return ok("ERR invalid key");
            }
            if st.store.del(org, key) {
                ok("OK")
            } else {
                ok("NIL")
            }
        }
        _ => ok("ERR unknown command"),
    }
}
