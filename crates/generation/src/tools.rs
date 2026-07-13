//! MCP Tool Definitions for Generation
//!
//! Defines the tool schemas for generate_image, generate_video,
//! generate_svg, and other generation tools.

/// Compile-time JSON literal helper — avoids `json!()` macro which internally uses `.unwrap()`.
fn json_literal(json_str: &str) -> serde_json::Value {
    serde_json::from_str(json_str).unwrap_or_else(|e| panic!("valid JSON literal: {e}"))
}

/// Get the schema for the generate_image tool
pub fn generate_image_schema() -> serde_json::Value {
    json_literal(
        r#"{
        "name": "generate_image",
        "description": "Generate an image from a text description. Uses local models (no cloud API). Supports various art styles, aspect ratios, and quality tiers.",
        "input_schema": {
            "type": "object",
            "properties": {
                "subject": {
                    "type": "string",
                    "description": "Description of what to generate (e.g., 'a sunset over mountains')"
                },
                "art_style": {
                    "type": "string",
                    "enum": ["photorealistic", "anime", "surreal", "vector", "pixel_art", "oil_painting", "watercolor", "sketch"],
                    "description": "Art style for the generated image",
                    "default": "photorealistic"
                },
                "aspect_ratio": {
                    "type": "string",
                    "enum": ["1:1", "16:9", "9:16", "4:3"],
                    "description": "Aspect ratio of the generated image",
                    "default": "1:1"
                },
                "quality_tier": {
                    "type": "string",
                    "enum": ["fast", "balanced", "quality"],
                    "description": "Quality tier (fast=20 steps, balanced=30, quality=50)",
                    "default": "balanced"
                },
                "negative_prompt": {
                    "type": "string",
                    "description": "What to avoid in the generated image"
                }
            },
            "required": ["subject"]
        }
    }"#,
    )
}

/// Get the schema for the generate_video tool
pub fn generate_video_schema() -> serde_json::Value {
    json_literal(
        r#"{
        "name": "generate_video",
        "description": "Generate a short video from a text description. Uses local models (no cloud API). Supports various durations and resolutions.",
        "input_schema": {
            "type": "object",
            "properties": {
                "subject": {
                    "type": "string",
                    "description": "Description of the video to generate (e.g., 'a cat walking across a room')"
                },
                "duration": {
                    "type": "string",
                    "enum": ["5s", "10s"],
                    "description": "Video duration",
                    "default": "5s"
                },
                "resolution": {
                    "type": "string",
                    "enum": ["480p", "720p"],
                    "description": "Video resolution",
                    "default": "480p"
                },
                "style": {
                    "type": "string",
                    "enum": ["cinematic", "anime", "realistic"],
                    "description": "Video style",
                    "default": "cinematic"
                }
            },
            "required": ["subject"]
        }
    }"#,
    )
}

/// Get the schema for the generate_svg tool
pub fn generate_svg_schema() -> serde_json::Value {
    json_literal(
        r#"{
        "name": "generate_svg",
        "description": "Generate an SVG image via LLM. Zero VRAM required. Best for diagrams, icons, charts, and simple illustrations.",
        "input_schema": {
            "type": "object",
            "properties": {
                "subject": {
                    "type": "string",
                    "description": "Description of what to generate (e.g., 'a flowchart showing login process')"
                },
                "style": {
                    "type": "string",
                    "enum": ["diagram", "icon", "chart", "illustration", "vector"],
                    "description": "SVG style",
                    "default": "illustration"
                },
                "format": {
                    "type": "string",
                    "enum": ["png", "svg"],
                    "description": "Output format (png renders SVG to PNG, svg returns raw SVG)",
                    "default": "png"
                }
            },
            "required": ["subject"]
        }
    }"#,
    )
}

/// Get the schema for the edit_image tool
pub fn edit_image_schema() -> serde_json::Value {
    json_literal(
        r#"{
        "name": "edit_image",
        "description": "Edit an existing image with a text instruction. Can change colors, add/remove objects, apply style transfers.",
        "input_schema": {
            "type": "object",
            "properties": {
                "image_id": {
                    "type": "string",
                    "description": "ID of the image to edit"
                },
                "instruction": {
                    "type": "string",
                    "description": "Edit instruction (e.g., 'make the sky blue', 'add a cat in the foreground')"
                }
            },
            "required": ["image_id", "instruction"]
        }
    }"#,
    )
}

/// Get the schema for the image_to_video tool
pub fn image_to_video_schema() -> serde_json::Value {
    json_literal(
        r#"{
        "name": "image_to_video",
        "description": "Convert a static image into a short video with motion.",
        "input_schema": {
            "type": "object",
            "properties": {
                "image_id": {
                    "type": "string",
                    "description": "ID of the source image"
                },
                "motion": {
                    "type": "string",
                    "description": "Motion description (e.g., 'camera slowly zooms in', 'gentle wind blowing')"
                },
                "duration": {
                    "type": "string",
                    "enum": ["5s", "10s"],
                    "description": "Video duration",
                    "default": "5s"
                }
            },
            "required": ["image_id", "motion"]
        }
    }"#,
    )
}

/// Get the schema for the upscale_image tool
pub fn upscale_image_schema() -> serde_json::Value {
    json_literal(
        r#"{
        "name": "upscale_image",
        "description": "Upscale an image to higher resolution using AI upscaling.",
        "input_schema": {
            "type": "object",
            "properties": {
                "image_id": {
                    "type": "string",
                    "description": "ID of the image to upscale"
                },
                "scale": {
                    "type": "string",
                    "enum": ["2x", "4x"],
                    "description": "Upscale factor",
                    "default": "2x"
                }
            },
            "required": ["image_id"]
        }
    }"#,
    )
}

/// Get the schema for the remove_background tool
pub fn remove_background_schema() -> serde_json::Value {
    json_literal(
        r#"{
        "name": "remove_background",
        "description": "Remove the background from an image, creating a transparent PNG.",
        "input_schema": {
            "type": "object",
            "properties": {
                "image_id": {
                    "type": "string",
                    "description": "ID of the image to process"
                }
            },
            "required": ["image_id"]
        }
    }"#,
    )
}
