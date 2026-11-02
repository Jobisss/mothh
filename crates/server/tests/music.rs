//! Integração com PostgreSQL, sem acesso a YouTube ou LiveKit.
use protocol::{ConversationId, MusicCommand, MusicSnapshot, ServerMessage, UserId};
use server::{AppState, calls::LiveKitConfig, music::MusicConfig};
use sqlx::PgPool;
use uuid::Uuid;

fn state(pool: PgPool) -> AppState {
    AppState::new(pool)
        .with_livekit(LiveKitConfig {
            url: "ws://127.0.0.1:7880".into(),
            api_key: "test".into(),
            api_secret: "test-secret".into(),
        })
        .with_music_config(MusicConfig {
            worker: "mothh-test-nonexistent-worker".into(),
            yt_dlp: "mothh-test-nonexistent-resolver".into(),
            ffmpeg: "mothh-test-nonexistent-decoder".into(),
        })
}

async fn user(pool: &PgPool, name: &str) -> UserId {
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO users (username, password_hash) VALUES ($1, 'unused-test-hash') RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap();
    UserId(id)
}

fn snapshot(message: ServerMessage) -> MusicSnapshot {
    match message {
        ServerMessage::MusicSnapshot { snapshot } => snapshot,
        other => panic!("expected snapshot, got {other:?}"),
    }
}

#[sqlx::test]
async fn access_control_and_listener_tokens(pool: PgPool) {
    let owner = user(&pool, "owner").await;
    let member = user(&pool, "member").await;
    let outsider = user(&pool, "outsider").await;
    let app = state(pool);
    let group = app.create_group(owner, "music").await.unwrap().id;
    app.invite(owner, group, "member").await.unwrap();
    assert!(matches!(
        app.music_command(outsider, group, MusicCommand::Listen, None)
            .await,
        ServerMessage::MusicError { .. }
    ));
    assert!(matches!(
        app.music_command(member, group, MusicCommand::Stop, None)
            .await,
        ServerMessage::MusicError { .. }
    ));
    assert!(matches!(
        app.music_command(member, group, MusicCommand::Listen, None)
            .await,
        ServerMessage::MusicToken { .. }
    ));
    let state = snapshot(
        app.music_command(owner, group, MusicCommand::GetSnapshot, None)
            .await,
    );
    assert_eq!(state.controller_id, Some(owner));
    let stale = app
        .music_command(owner, group, MusicCommand::Clear, Some(state.revision + 1))
        .await;
    assert!(matches!(stale, ServerMessage::MusicError { .. }));
    app.music_command(owner, group, MusicCommand::Stop, None)
        .await;
}

#[sqlx::test]
async fn enqueue_is_idempotent_and_members_only_remove_their_requests(pool: PgPool) {
    let owner = user(&pool, "owner").await;
    let member = user(&pool, "member").await;
    let app = state(pool);
    let group = app.create_group(owner, "music").await.unwrap().id;
    app.invite(owner, group, "member").await.unwrap();
    let command = MusicCommand::Enqueue {
        request_id: Uuid::new_v4(),
        query: "test track".into(),
    };
    let first = snapshot(app.music_command(owner, group, command.clone(), None).await);
    let second = snapshot(app.music_command(owner, group, command, None).await);
    assert_eq!(first.queue.len(), 1);
    assert_eq!(second.queue.len(), 1);
    assert_eq!(first.queue[0].item_id, second.queue[0].item_id);
    let remove = MusicCommand::Remove {
        item_id: first.queue[0].item_id,
    };
    assert!(matches!(
        app.music_command(member, group, remove.clone(), None).await,
        ServerMessage::MusicError { .. }
    ));
    assert!(
        snapshot(app.music_command(owner, group, remove, None).await)
            .queue
            .is_empty()
    );
    app.music_command(owner, group, MusicCommand::Stop, None)
        .await;
}

#[sqlx::test]
async fn leaving_rotates_music_room_and_revokes_new_token_requests(pool: PgPool) {
    let owner = user(&pool, "owner").await;
    let member = user(&pool, "member").await;
    let app = state(pool);
    let group: ConversationId = app.create_group(owner, "music").await.unwrap().id;
    app.invite(owner, group, "member").await.unwrap();
    let before = snapshot(
        app.music_command(owner, group, MusicCommand::GetSnapshot, None)
            .await,
    );
    app.leave(member, group).await.unwrap();
    assert!(matches!(
        app.music_command(member, group, MusicCommand::Listen, None)
            .await,
        ServerMessage::MusicError { .. }
    ));
    let after = snapshot(
        app.music_command(owner, group, MusicCommand::GetSnapshot, None)
            .await,
    );
    assert_ne!(before.session_id, after.session_id);
    app.music_command(owner, group, MusicCommand::Stop, None)
        .await;
}
