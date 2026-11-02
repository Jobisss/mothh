use protocol::{ListeningActivity, ServerMessage, UserId};
use server::AppState;
use sqlx::PgPool;
use uuid::Uuid;

async fn user(pool: &PgPool, name: &str) -> UserId {
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO users (username, password_hash) VALUES ($1, 'test') RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap();
    UserId(id)
}

#[sqlx::test]
async fn listening_obeys_profile_access_and_can_be_cleared(pool: PgPool) {
    let owner = user(&pool, "owner").await;
    let friend = user(&pool, "friend").await;
    let outsider = user(&pool, "outsider").await;
    let state = AppState::new(pool);
    let group = state.create_group(owner, "friends").await.unwrap().id;
    state.invite(owner, group, "friend").await.unwrap();
    let activity = ListeningActivity {
        title: "Tall Trees".into(),
        artist: "i-dle".into(),
        position_ms: 20_000,
        duration_ms: 165_000,
    };
    state
        .set_listening_activity(owner, Some(activity.clone()))
        .await
        .unwrap();
    let ServerMessage::Profile { profile } = state.profile(friend, owner).await.unwrap() else {
        panic!()
    };
    assert_eq!(profile.listening.unwrap().title, activity.title);
    assert!(state.profile(outsider, owner).await.is_err());
    let invalid = ListeningActivity {
        title: "x".repeat(513),
        ..activity
    };
    assert!(
        state
            .set_listening_activity(owner, Some(invalid))
            .await
            .is_err()
    );
    state.set_listening_activity(owner, None).await.unwrap();
    let ServerMessage::Profile { profile } = state.profile(friend, owner).await.unwrap() else {
        panic!()
    };
    assert!(profile.listening.is_none());
}
