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

use crate::{Client, Result};

const CHAT: &str = "https://apis.roblox.com/platform-chat-api/v1";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Conversation {
    pub id: String,
    #[serde(alias = "title")]
    pub name: String,
    /// "OneToOne" or "Group".
    pub conversation_type: String,
    pub participants: Option<Vec<Participant>>,
    pub last_updated: Option<String>,
    pub unread_message_count: i64,
    pub latest_messages: Option<Vec<Message>>,
}

impl Conversation {
    /// For a one-to-one chat the useful title is the *other* person, not the
    /// conversation's own name (which is often empty or your own id).
    pub fn display_title(&self, me: i64) -> String {
        if !self.name.is_empty() && self.conversation_type != "OneToOne" {
            return self.name.clone();
        }
        self.participants
            .as_ref()
            .and_then(|ps| ps.iter().find(|p| p.user_id != me))
            .map(|p| {
                if p.display_name.is_empty() {
                    p.name.clone()
                } else {
                    p.display_name.clone()
                }
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if self.name.is_empty() {
                    "Conversation".into()
                } else {
                    self.name.clone()
                }
            })
    }

    pub fn other_participant(&self, me: i64) -> Option<i64> {
        self.participants
            .as_ref()?
            .iter()
            .find(|p| p.user_id != me)
            .map(|p| p.user_id)
    }

    pub fn preview(&self) -> String {
        self.latest_messages
            .as_ref()
            .and_then(|m| m.first())
            .map(|m| m.content.clone())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Participant {
    #[serde(alias = "id", alias = "userId")]
    pub user_id: i64,
    #[serde(alias = "username")]
    pub name: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Message {
    pub id: String,
    #[serde(alias = "messageContent", alias = "text")]
    pub content: String,
    #[serde(alias = "senderUserId")]
    pub sender_id: i64,
    #[serde(alias = "createdTime", alias = "sentTime")]
    pub created: String,
    pub message_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationsResponse {
    #[serde(default = "Vec::new", alias = "data")]
    conversations: Vec<Conversation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    let url = format!(
        "{CHAT}/get-conversation-messages?conversationId={conversation_id}&pageSize={page_size}"
    );
    let resp: MessagesResponse = client.get_json(&url).await?;
    Ok(resp.messages)
}

/// Send a message. Note the plural endpoint name — the singular 404s.
pub async fn send(client: &Client, conversation_id: &str, text: &str) -> Result<()> {
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
        .map(|c| c.id)
        .ok_or_else(|| crate::Error::Api("Roblox created no conversation".into()))
}

pub async fn mark_read(client: &Client, conversation_id: &str) -> Result<()> {
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

    fn one_to_one() -> Conversation {
        Conversation {
            id: "c1".into(),
            name: String::new(),
            conversation_type: "OneToOne".into(),
            participants: Some(vec![
                Participant { user_id: 1, name: "me".into(), display_name: "Me".into() },
                Participant { user_id: 2, name: "henry".into(), display_name: "UsedHenry06".into() },
            ]),
            last_updated: None,
            unread_message_count: 3,
            latest_messages: Some(vec![Message {
                id: "m1".into(),
                content: "yeah im on now".into(),
                sender_id: 2,
                created: String::new(),
                message_type: None,
            }]),
        }
    }

    #[test]
    fn one_to_one_title_is_the_other_person() {
        // Not "Me", and not the conversation's own empty name.
        assert_eq!(one_to_one().display_title(1), "UsedHenry06");
        assert_eq!(one_to_one().display_title(2), "Me");
    }

    #[test]
    fn group_conversation_keeps_its_own_name() {
        let mut c = one_to_one();
        c.conversation_type = "Group".into();
        c.name = "Paradoxum Fans".into();
        assert_eq!(c.display_title(1), "Paradoxum Fans");
    }

    #[test]
    fn a_nameless_conversation_with_no_participants_still_renders() {
        let c = Conversation {
            id: "x".into(),
            conversation_type: "OneToOne".into(),
            ..Default::default()
        };
        assert_eq!(c.display_title(1), "Conversation");
        assert!(c.other_participant(1).is_none());
    }

    #[test]
    fn falls_back_to_username_when_display_name_is_blank() {
        let mut c = one_to_one();
        if let Some(ps) = c.participants.as_mut() {
            ps[1].display_name = String::new();
        }
        assert_eq!(c.display_title(1), "henry");
    }

    #[test]
    fn preview_uses_the_latest_message() {
        assert_eq!(one_to_one().preview(), "yeah im on now");
        assert_eq!(Conversation::default().preview(), "");
    }

    #[test]
    fn unread_totals_across_conversations() {
        let mut b = one_to_one();
        b.unread_message_count = 7;
        assert_eq!(total_unread(&[one_to_one(), b]), 10);
        assert_eq!(total_unread(&[]), 0);
    }

    #[test]
    fn message_parsing_accepts_the_field_aliases() {
        // Roblox has shipped more than one spelling of these fields; the
        // aliases mean a rename does not blank the chat.
        let m: Message =
            serde_json::from_str(r#"{"id":"1","messageContent":"hi","senderUserId":5,"sentTime":"t"}"#)
                .unwrap();
        assert_eq!(m.content, "hi");
        assert_eq!(m.sender_id, 5);
        assert_eq!(m.created, "t");
    }

    #[test]
    fn conversation_list_accepts_data_or_conversations_key() {
        let a: ConversationsResponse =
            serde_json::from_str(r#"{"conversations":[{"id":"1"}]}"#).unwrap();
        assert_eq!(a.conversations.len(), 1);

        let b: ConversationsResponse = serde_json::from_str(r#"{"data":[{"id":"2"}]}"#).unwrap();
        assert_eq!(b.conversations.len(), 1);

        let c: ConversationsResponse = serde_json::from_str("{}").unwrap();
        assert!(c.conversations.is_empty());
    }
}
