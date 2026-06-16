use crate::core::model::error::PoleError;
use crate::traffic::policy::req::MirrorRequest;
use pole_specification::v1::MirrorDestination;

#[async_trait::async_trait]
pub trait MirrorSender: Send + Sync + 'static {
    async fn send(
        &self,
        destination: MirrorDestination,
        request: MirrorRequest,
    ) -> Result<(), PoleError>;
}
