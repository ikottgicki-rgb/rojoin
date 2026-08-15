//! Direct messages, on Roblox's current chat service.
//!
//! **`chat.roblox.com` is dead** — every path under it answers 404. The live
//! service is `apis.roblox.com/platform-chat-api/v1`, and its verb/name pairs
//! are not guessable, so they are recorded here from probing:
//!
//! | Method | Path | Purpose |
//! |--------|------|---------|
//! | GET  | `metadata`                  | feature flags |
//! | GET  | `get-user-conversations`    | the conversation list |
//! | GET  | `get-conversation-messages` | message history (GET only) |
//! | POST | `get-conversations`         | batch lookup by id |
//! | POST | `send-messages`             | send (plural) |
//! | POST | `create-conversations`      | start a new one (plural) |
//! | POST | `update-conversations`      | mark read, rename |
//!
//! Note the pluralisation: `send-messages`, not `send-message`. The singular
//! forms all 404.

use serde::Deserialize;

use crate::models::{flexible_id, null_default};
use crate::{Client, Result};

const CHAT: &str = "https://apis.roblox.com/platform-chat-api/v1";

/// A conversation as `get-user-conversations` actually returns it.
///
/// The payload is **snake_case**, unlike the rest of the Roblox API, so there
/// is no `rename_all` here — the Rust field names already match.
///
/// Two shapes matter and neither is obvious:
///   * `id` is **null** when `source == "friends"`. Those are friends you have
///     never messaged: Roblox lists them as potential conversations that do
///     not exist yet. Calling any conversation endpoint with a null id gives
///     `400 empty conversationId`.
///   * Participants arrive as `participant_user_ids` plus a `user_data` map
///     keyed by stringified user id — there is no `participants` array.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Conversation {
    /// None for a friend you have not messaged yet.
    pub id: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub source: String,
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: String,
    /// Already the display title Roblox wants shown.
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub participant_user_ids: Vec<i64>,
    #[serde(deserialize_with = "null_default")]
    pub user_data: std::collections::HashMap<String, UserData>,
    #[serde(deserialize_with = "null_default")]
    pub messages: Vec<Message>,
    #[serde(deserialize_with = "null_default")]
    pub unread_message_count: i64,
    pub updated_at: Option<String>,
    // Roblox sends this as null on conversations that have never been used.
    #[serde(deserialize_with = "null_default")]
    pub sort_index: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct UserData {
    #[serde(deserialize_with = "null_default")]
    pub id: i64,
    /// `name` and `display_name` are routinely null; `combined_name` is what
    /// Roblox actually renders.
    pub name: Option<String>,
    pub display_name: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub combined_name: String,
    #[serde(deserialize_with = "null_default")]
    pub is_verified: bool,
}

impl Conversation {
    /// A conversation you can actually fetch or post to.
    pub fn is_real(&self) -> bool {
        self.id.as_deref().is_some_and(|id| !id.is_empty())
    }

    pub fn display_title(&self, me: i64) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        self.other_participant(me)
            .and_then(|id| self.user_data.get(&id.to_string()))
            .map(|u| u.combined_name.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Conversation".into())
    }

    pub fn other_participant(&self, me: i64) -> Option<i64> {
        self.participant_user_ids
            .iter()
            .copied()
            .find(|id| *id != me)
            // A self-conversation still needs someone to show.
            .or_else(|| self.participant_user_ids.first().copied())
    }

    pub fn preview(&self) -> String {
        self.messages.first().map(|m| m.content.clone()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Message {
    pub id: Option<String>,
    #[serde(alias = "message_content", alias = "text", deserialize_with = "null_default")]
    pub content: String,
    #[serde(
        alias = "sender_user_id",
        alias = "sender_target_id",
        deserialize_with = "null_default"
    )]
    pub sender_id: i64,
    #[serde(alias = "created_at", alias = "sent_at")]
    pub created: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConversationsResponse {
    #[serde(default = "Vec::new", alias = "data")]
    conversations: Vec<Conversation>,
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    #[serde(default = "Vec::new", alias = "data")]
    messages: Vec<Message>,
}

/// The conversation list, newest activity first.
pub async fn conversations(client: &Client, page_size: u32) -> Result<Vec<Conversation>> {
    let url = format!("{CHAT}/get-user-conversations?pageNumber=1&pageSize={page_size}");

    let resp: ConversationsResponse = client.get_json(&url).await?;
    Ok(resp.conversations)
}

/// Message history for one conversation. Newest first, as Roblox returns it.
pub async fn messages(
    client: &Client,
    conversation_id: &str,
    page_size: u32,
) -> Result<Vec<Message>> {
    // A friend-derived conversation has no id yet; there is nothing to fetch.
    if conversation_id.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!(
        "{CHAT}/get-conversation-messages?conversationId={conversation_id}&pageSize={page_size}"
    );
    let resp: MessagesResponse = client.get_json(&url).await?;
    Ok(resp.messages)
}

/// Send a message. Note the plural endpoint name — the singular 404s.
pub async fn send(client: &Client, conversation_id: &str, text: &str) -> Result<()> {
    if conversation_id.is_empty() {
        return Err(crate::Error::Api(
            "that conversation does not exist yet — send from the profile to start it".into(),
        ));
    }
    let body = serde_json::json!({
        "conversationId": conversation_id,
        "messages": [{ "content": text }],
    });
    let _: serde_json::Value = client.post_json(&format!("{CHAT}/send-messages"), &body).await?;
    Ok(())
}

/// Start (or find) a one-to-one conversation with someone.
pub async fn create_with(client: &Client, user_id: i64) -> Result<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Created {
        #[serde(default)]
        conversations: Vec<Conversation>,
    }

    let body = serde_json::json!({
        "conversations": [{
            "participantUserIds": [user_id],
            "conversationType": "OneToOne",
        }],
    });

    let created: Created = client
        .post_json(&format!("{CHAT}/create-conversations"), &body)
        .await?;

    created
        .conversations
        .into_iter()
        .next()
        .and_then(|c| c.id)
        .ok_or_else(|| crate::Error::Api("Roblox created no conversation".into()))
}

pub async fn mark_read(client: &Client, conversation_id: &str) -> Result<()> {
    if conversation_id.is_empty() {
        return Ok(());
    }
    let body = serde_json::json!({
        "conversations": [{ "conversationId": conversation_id, "hasUnreadMessages": false }],
    });
    let _: serde_json::Value = client
        .post_json(&format!("{CHAT}/update-conversations"), &body)
        .await?;
    Ok(())
}

pub fn total_unread(conversations: &[Conversation]) -> i64 {
    conversations.iter().map(|c| c.unread_message_count).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim from a live `get-user-conversations` response.
    const REAL: &str = r#"{"conversations":[
        {"source":"friends","id":null,"type":"one_to_one","name":"Goopchan",
         "is_default_name":true,"created_by":null,
         "participant_user_ids":[343552312,3626863294],
         "user_data":{"3626863294":{"id":3626863294,"name":null,"display_name":null,
                      "combined_name":"Goopchan","is_verified":false}},
         "messages":[],"unread_message_count":0,
         "updated_at":"2026-06-15T16:47:54.255Z","sort_index":1781542074255},
        {"source":"channels","id":"0f8dd898-e5a4-517f-bf30-1abc2b1c9af7",
         "type":"one_to_one","name":"pantera_nera99","created_by":1528509367,
         "participant_user_ids":[343552312,1528509367],
         "user_data":{"1528509367":{"id":1528509367,"name":null,"display_name":null,
                      "combined_name":"pantera_nera99","is_verified":false}},
         "messages":[],"unread_message_count":3,
         "updated_at":"2026-04-09T23:13:36.309Z","sort_index":1775776416309}
    ]}"#;

    fn parsed() -> Vec<Conversation> {
        serde_json::from_str::<ConversationsResponse>(REAL).unwrap().conversations
    }

    #[test]
    fn parses_the_real_snake_case_payload() {
        let c = parsed();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].name, "Goopchan");
        assert_eq!(c[1].unread_message_count, 3);
        assert_eq!(c[1].kind, "one_to_one");
    }

    #[test]
    fn a_friend_conversation_with_a_null_id_is_not_real() {
        // These are friends you have never messaged. Calling any conversation
        // endpoint with them returns 400 "empty conversationId".
        let c = parsed();
        assert!(!c[0].is_real(), "null id must not be treated as fetchable");
        assert!(c[1].is_real());
        assert_eq!(c[1].id.as_deref(), Some("0f8dd898-e5a4-517f-bf30-1abc2b1c9af7"));
    }

    #[test]
    fn the_other_participant_is_found_for_avatars() {
        // This is what was blank: participants come as ids plus a user_data
        // map, not as a participants array.
        let c = parsed();
        assert_eq!(c[0].other_participant(343552312), Some(3626863294));
        assert_eq!(c[1].other_participant(343552312), Some(1528509367));
    }

    #[test]
    fn titles_fall_back_to_combined_name_when_name_is_blank() {
        let mut c = parsed().remove(0);
        c.name = String::new();
        // name and display_name are null in the real payload; combined_name is
        // the only usable one.
        assert_eq!(c.display_title(343552312), "Goopchan");
    }

    #[test]
    fn every_nullable_field_survives_being_null() {
        // Roblox nulls these freely, and a single one used to fail the whole
        // conversation list.
        let json = r#"{"conversations":[{"source":null,"id":null,"type":null,
            "name":null,"participant_user_ids":null,"user_data":null,
            "messages":null,"unread_message_count":null,"updated_at":null,
            "sort_index":null}]}"#;
        let c = serde_json::from_str::<ConversationsResponse>(json).unwrap().conversations;
        assert_eq!(c.len(), 1);
        assert!(!c[0].is_real());
        assert_eq!(c[0].display_title(1), "Conversation");
    }

    #[test]
    fn a_conversation_with_nothing_usable_still_renders() {
        let c = Conversation::default();
        assert_eq!(c.display_title(1), "Conversation");
        assert!(!c.is_real());
        assert_eq!(c.preview(), "");
    }

    #[test]
    fn unread_totals_across_conversations() {
        assert_eq!(total_unread(&parsed()), 3);
        assert_eq!(total_unread(&[]), 0);
    }

    #[tokio::test]
    async fn sending_to_a_nonexistent_conversation_explains_itself() {
        let client = Client::new().unwrap();
        let err = send(&client, "", "hello").await.unwrap_err();
        assert!(err.to_string().contains("does not exist yet"));
    }
}
