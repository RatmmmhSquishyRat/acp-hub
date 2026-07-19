use super::*;

pub(crate) fn validate_prompt_capabilities(
    agent_id: &str,
    caps: &AgentCapabilities,
    prompt: &[ContentBlock],
) -> Result<(), HubError> {
    for block in prompt {
        let (supported, required_capability) = match block {
            ContentBlock::Image(_) => (caps.prompt_capabilities.image, "prompt_capabilities.image"),
            ContentBlock::Audio(_) => (caps.prompt_capabilities.audio, "prompt_capabilities.audio"),
            ContentBlock::Resource(_) => (
                caps.prompt_capabilities.embedded_context,
                "prompt_capabilities.embedded_context",
            ),
            ContentBlock::Text(_) | ContentBlock::ResourceLink(_) => continue,
            _ => {
                return Err(HubError::UnsupportedCapability {
                    endpoint: agent_id.to_string(),
                    operation: "session/prompt",
                    required_capability: "prompt_capabilities.unknown_content",
                });
            }
        };
        if !supported {
            return Err(HubError::UnsupportedCapability {
                endpoint: agent_id.to_string(),
                operation: "session/prompt",
                required_capability,
            });
        }
    }
    Ok(())
}
