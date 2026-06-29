// Boundary: Provides compiler-guided ownership and Result exercises; it must not implement product behavior.

#[derive(Debug)]
struct ModelRequest {
    model: String,
    prompt: String,
}

impl ModelRequest {
    fn new(model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
        }
    }

    fn summary(&self) -> String {
        format!("{}: {}", self.model, self.prompt)
    }
}

fn validate_request(request: &ModelRequest) -> Result<(), String> {
    if request.prompt.trim().is_empty() {
        return Err("prompt must not be empty".to_owned());
    }

    Ok(())
}

fn send_request(request: ModelRequest) -> Result<String, String> {
    validate_request(&request)?;
    Ok(format!("sent {}", request.summary()))
}

fn main() -> Result<(), String> {
    let request = ModelRequest::new("deepseek-chat", "Explain the Codex turn loop");

    // `summary` only borrows `request`, so ownership stays in this scope.
    println!("prepared {}", request.summary());

    // `send_request` takes ownership. `request` cannot be used after this line.
    let response = send_request(request)?;
    println!("{response}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_empty_prompt() {
        let request = ModelRequest::new("deepseek-chat", "   ");
        assert_eq!(
            validate_request(&request),
            Err("prompt must not be empty".to_owned())
        );
    }

    #[test]
    fn sends_a_valid_request() {
        let request = ModelRequest::new("deepseek-chat", "hello");
        assert_eq!(
            send_request(request),
            Ok("sent deepseek-chat: hello".to_owned())
        );
    }
}
