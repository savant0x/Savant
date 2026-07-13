//! Routing agent — lightweight LLM call for profile selection.
//!
//! Given a task description, selects the best profile from the available set.
//! Uses the same model as specialists — routing is a single LLM call with
//! a short system prompt listing available profiles and their descriptions.
//! Falls back to "general" profile if routing fails or is ambiguous.
