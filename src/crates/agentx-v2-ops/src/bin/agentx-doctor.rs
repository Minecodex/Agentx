use agentx_v2_ops::{Plane, doctor};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    agentx_service_kit::install_tls_provider();
    doctor(Plane::parse(std::env::args().nth(1))?).await
}
