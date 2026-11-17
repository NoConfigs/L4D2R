use reqwest::{Client, Response};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use steam_vent::{
    Connection, ConnectionTrait,
    proto::steammessages_clientserver_appinfo::{
        CMsgClientPICSAccessTokenRequest, CMsgClientPICSAccessTokenResponse,
        CMsgClientPICSProductInfoRequest, CMsgClientPICSProductInfoResponse,
        cmsg_client_picsproduct_info_request::AppInfo,
    },
};

use crate::{Error, web_api::content_service::CDNServer};

use super::depot_chunk;

#[derive(Debug)]
pub(crate) struct InnerClient {
    pub connection: Arc<Connection>,
    web_client: Client,
    pub servers: Vec<CDNServer>,
    next_server: AtomicUsize,
}

impl InnerClient {
    pub fn new(connection: Arc<Connection>) -> Self {
        Self {
            connection,
            web_client: Client::builder()
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(45))
                .tcp_nodelay(true)
                .build()
                .expect("HTTP client"),
            servers: Vec::new(),
            next_server: AtomicUsize::new(0),
        }
    }

    pub fn cell_id(&self) -> u32 {
        self.connection.cell_id()
    }

    fn get_server(&self) -> Result<&CDNServer, Error> {
        let candidates: Vec<_> = self
            .servers
            .iter()
            .filter(|s| s.r#type == "SteamCache" || s.r#type == "CDN")
            .take(12)
            .collect();
        if candidates.is_empty() {
            return Err(Error::Network("No Steam download servers available".into()));
        }
        Ok(candidates[self.next_server.load(Ordering::Relaxed) % candidates.len()])
    }

    pub async fn get_product_info(
        &self,
        app_ids: Vec<u32>,
    ) -> Result<CMsgClientPICSProductInfoResponse, Error> {
        let tokens: CMsgClientPICSAccessTokenResponse = self
            .connection
            .job(CMsgClientPICSAccessTokenRequest {
                appids: app_ids,
                ..Default::default()
            })
            .await?;

        let product_info: CMsgClientPICSProductInfoResponse = self
            .connection
            .job(CMsgClientPICSProductInfoRequest {
                apps: tokens
                    .app_access_tokens
                    .into_iter()
                    .map(|app_token| AppInfo {
                        appid: app_token.appid,
                        access_token: app_token.access_token,
                        ..Default::default()
                    })
                    .collect::<Vec<AppInfo>>(),
                meta_data_only: Some(false),
                ..Default::default()
            })
            .await?;
        Ok(product_info)
    }

    pub async fn remote_cmd<C: AsRef<str>, A: AsRef<str>>(
        &self,
        command: C,
        args: A,
        manifest_request_code: Option<u64>,
    ) -> Result<Response, Error> {
        let mut last = Error::Network("Steam download servers unavailable".into());
        for attempt in 0..4 {
            let server = self.get_server()?;
            let mut url = format!(
                "{}://{}:{}/{}/{}",
                if server.https { "https" } else { "http" },
                server.host,
                server.port,
                command.as_ref(),
                args.as_ref()
            );
            if let Some(manifest_request_code) = manifest_request_code {
                url.push('/');
                url.push_str(manifest_request_code.to_string().as_str());
            }
            match self.web_client.get(url).send().await {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) => last = Error::HttpStatus(response.status()),
                Err(_) => {
                    last =
                        Error::Network("Steam server connection timed out or disconnected".into())
                }
            }
            self.next_server.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(250 * (attempt + 1))).await;
        }
        Err(last)
    }

    pub async fn get_chunk(
        &self,
        depot_id: u32,
        depot_key: [u8; 32],
        chunk_id: String,
    ) -> Result<Vec<u8>, Error> {
        let mut last = Error::Network("Chunk download interrupted".into());
        for attempt in 0..4 {
            let result = async {
                let response = self
                    .remote_cmd("depot", format!("{depot_id}/chunk/{chunk_id}"), None)
                    .await?;
                if !response.status().is_success() {
                    return Err(Error::HttpStatus(response.status()));
                }

                let mut bytes = response
                    .bytes()
                    .await
                    .map_err(|_| Error::Network("Chunk transfer timed out or disconnected".into()))?
                    .to_vec();
                depot_chunk::decrypt_and_decompress(&mut bytes[..], depot_key).await
            }
            .await;
            match result {
                Ok(data) => return Ok(data),
                Err(error) => last = error,
            }
            self.next_server.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(300 * (attempt + 1))).await;
        }
        Err(last)
    }
}
