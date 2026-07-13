/// Enum representing distinct agent loop events.
pub enum AgentEvent {
    Thought(String),
    Action {
        name: String,
        args: String,
    },
    Observation(String),
    FinalAnswer(String),
    FinalAnswerChunk(String),
    Reflection(String),
    StatusUpdate(String), // Internal status heartbeats
    /// Emitted when a session turn begins. Signals the start of a new user/agent interaction.
    SessionStart {
        session_id: String,
        turn_id: String,
    },
    /// Emitted when a session turn ends. Contains the tool calls made during this turn.
    TurnEnd {
        session_id: String,
        turn_id: String,
        turn_count: u64,
        tool_calls: Vec<String>,
    },
}
