//! Envio do código de recuperação de senha (V6-04).
//!
//! Ainda não há SMTP: o `LogMailer` escreve o código no log do servidor, e
//! quem opera o servidor repassa à pessoa por outro canal. Quando houver um
//! provedor, basta outra implementação de `Mailer` ligada em `main.rs`
//! (`AppState::with_mailer`); o resto do fluxo não muda.

use std::sync::Arc;

pub trait Mailer: Send + Sync {
    /// Entrega `code` a `email`. Não devolve erro: o pedido de código
    /// responde igual com ou sem conta, e a entrega é assíncrona por natureza.
    fn send_reset_code(&self, email: &str, code: &str);
}

/// Escreve o código no log, em vez de mandar e-mail.
pub struct LogMailer;

impl Mailer for LogMailer {
    fn send_reset_code(&self, email: &str, code: &str) {
        tracing::warn!(
            %email,
            %code,
            "código de recuperação de senha (sem SMTP configurado: repasse à pessoa)"
        );
    }
}

/// Guarda os códigos em memória, para os testes lerem.
#[derive(Default)]
pub struct CapturingMailer {
    pub sent: std::sync::Mutex<Vec<(String, String)>>,
}

impl CapturingMailer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// O último código mandado para `email`.
    pub fn code_for(&self, email: &str) -> Option<String> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(to, _)| to == email)
            .map(|(_, code)| code.clone())
    }
}

impl Mailer for CapturingMailer {
    fn send_reset_code(&self, email: &str, code: &str) {
        self.sent
            .lock()
            .unwrap()
            .push((email.to_string(), code.to_string()));
    }
}
