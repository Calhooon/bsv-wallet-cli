use anyhow::Result;

use crate::context::WalletContext;
use crate::server::{self, ServerConfig, TlsConfig};

pub async fn run(ctx: WalletContext, port: u16) -> Result<()> {
    let tls = match (
        std::env::var("TLS_CERT_PATH").ok(),
        std::env::var("TLS_KEY_PATH").ok(),
    ) {
        (Some(cert_path), Some(key_path)) => Some(TlsConfig {
            cert_path,
            key_path,
        }),
        _ => None,
    };

    let config = ServerConfig {
        auth_token: std::env::var("AUTH_TOKEN").ok(),
        tls,
        chain: ctx.chain,
    };
    let wallet_state = server::make_wallet_state(ctx.wallet);
    server::run(wallet_state, port, config).await?;
    Ok(())
}
