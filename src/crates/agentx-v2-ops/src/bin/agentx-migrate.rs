use agentx_v2_ops::{Plane, migrate};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    agentx_service_kit::install_tls_provider();
    migrate(Plane::parse(std::env::args().nth(1))?).await
}
