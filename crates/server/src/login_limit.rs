//! Limite de tentativas de login por usuário, guardado em memória.
//!
//! 5 senhas erradas em 15 minutos bloqueiam o usuário por 15 minutos. Fica em
//! memória por enquanto: reiniciar o servidor zera a contagem (Redis entra
//! depois, quando houver mais de um servidor).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// As regras do limite. Os testes usam prazos curtos para não esperar 15 minutos.
#[derive(Debug, Clone, Copy)]
pub struct LoginLimit {
    /// Quantos erros dentro da janela bloqueiam o usuário.
    pub max_failures: usize,
    pub window: Duration,
    pub lockout: Duration,
}

impl Default for LoginLimit {
    fn default() -> Self {
        Self {
            max_failures: 5,
            window: Duration::from_secs(15 * 60),
            lockout: Duration::from_secs(15 * 60),
        }
    }
}

/// Acima disso o mapa é limpo, para nomes inventados não encherem a memória.
const PRUNE_ABOVE: usize = 10_000;

#[derive(Debug, Default)]
struct Attempts {
    /// Horário de cada tentativa errada (ou ainda em andamento) dentro da janela.
    failures: VecDeque<Instant>,
    locked_until: Option<Instant>,
}

impl Attempts {
    fn forget_old(&mut self, now: Instant, window: Duration) {
        while self
            .failures
            .front()
            .is_some_and(|&at| now.duration_since(at) >= window)
        {
            self.failures.pop_front();
        }
        if self.locked_until.is_some_and(|until| now >= until) {
            self.locked_until = None;
        }
    }

    fn is_stale(&self) -> bool {
        self.failures.is_empty() && self.locked_until.is_none()
    }
}

#[derive(Debug, Default)]
pub struct LoginLimiter {
    limit: LoginLimit,
    users: Mutex<HashMap<String, Attempts>>,
}

impl LoginLimiter {
    pub fn new(limit: LoginLimit) -> Self {
        Self {
            limit,
            users: Mutex::default(),
        }
    }

    /// Chamado antes de conferir a senha. Já conta a tentativa como errada:
    /// assim várias conexões ao mesmo tempo não passam do limite enquanto o
    /// argon2 ainda está conferindo. `Err` traz quanto falta para desbloquear.
    pub fn begin(&self, username: &str, now: Instant) -> Result<(), Duration> {
        let mut users = self.users.lock().expect("mutex do limite de login");
        if users.len() > PRUNE_ABOVE {
            users.retain(|_, attempts| {
                attempts.forget_old(now, self.limit.window);
                !attempts.is_stale()
            });
        }
        let attempts = users.entry(username.to_owned()).or_default();
        attempts.forget_old(now, self.limit.window);
        if let Some(until) = attempts.locked_until {
            return Err(until - now);
        }
        attempts.failures.push_back(now);
        if attempts.failures.len() >= self.limit.max_failures {
            attempts.failures.clear();
            attempts.locked_until = Some(now + self.limit.lockout);
        }
        Ok(())
    }

    /// A senha estava certa: esquece os erros (e o bloqueio que a própria
    /// tentativa possa ter armado).
    pub fn succeeded(&self, username: &str) {
        self.users
            .lock()
            .expect("mutex do limite de login")
            .remove(username);
    }
}

/// "Tente de novo em 14 min." Arredonda para cima, para nunca prometer cedo demais.
pub fn locked_message(remaining: Duration) -> String {
    let secs = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    let wait = if secs < 60 {
        format!("{secs} s")
    } else {
        format!("{} min", secs.div_ceil(60))
    };
    format!("Muitas tentativas erradas. Tente de novo em {wait}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn five_failures_lock_for_fifteen_minutes() {
        let limiter = LoginLimiter::default();
        let start = Instant::now();
        for _ in 0..5 {
            assert!(limiter.begin("jobiss", start).is_ok());
        }
        assert_eq!(limiter.begin("jobiss", start + MIN), Err(14 * MIN));
        // Outro usuário não é afetado.
        assert!(limiter.begin("nina", start + MIN).is_ok());
        // Passado o prazo, volta a aceitar.
        assert!(limiter.begin("jobiss", start + 15 * MIN).is_ok());
    }

    #[test]
    fn failures_outside_the_window_do_not_count() {
        let limiter = LoginLimiter::default();
        let start = Instant::now();
        for i in 0..4 {
            assert!(limiter.begin("jobiss", start + i * MIN).is_ok());
        }
        // A primeira tentativa já saiu da janela de 15 minutos.
        assert!(limiter.begin("jobiss", start + 15 * MIN).is_ok());
        assert!(limiter.begin("jobiss", start + 15 * MIN).is_ok());
        assert!(limiter.begin("jobiss", start + 15 * MIN).is_err());
    }

    #[test]
    fn success_clears_the_failures() {
        let limiter = LoginLimiter::default();
        let now = Instant::now();
        for _ in 0..4 {
            limiter.begin("jobiss", now).unwrap();
        }
        limiter.succeeded("jobiss");
        for _ in 0..4 {
            limiter.begin("jobiss", now).unwrap();
        }
        assert!(limiter.begin("jobiss", now).is_ok());
        assert!(limiter.begin("jobiss", now).is_err());
    }

    #[test]
    fn message_rounds_up() {
        assert_eq!(
            locked_message(Duration::from_millis(14 * 60_000 + 1)),
            "Muitas tentativas erradas. Tente de novo em 15 min."
        );
        assert_eq!(
            locked_message(Duration::from_millis(9_200)),
            "Muitas tentativas erradas. Tente de novo em 10 s."
        );
    }
}
