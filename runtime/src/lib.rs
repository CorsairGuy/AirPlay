//! Composes AirPlay services; applications control lifecycle through this API.
pub mod lifecycle;
#[cfg(test)]
mod mrp_path_tests;
pub mod pipeline;
pub mod relay;
pub mod routing;

/// One independently owned receiver instance.
pub struct AirPlay {
    receiver: lifecycle::receiver::Receiver,
}
impl AirPlay {
    pub async fn start(options: lifecycle::receiver::Options) -> anyhow::Result<Self> {
        Ok(Self {
            receiver: lifecycle::receiver::start(options).await?,
        })
    }
    pub fn receiver(&self) -> &lifecycle::receiver::Receiver {
        &self.receiver
    }
    pub async fn stop(self) {
        self.receiver.stop().await;
    }
}
