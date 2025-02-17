use std::fs;
use std::sync::Arc;

use anyhow::{anyhow, Error, Result};
use breez_sdk_core::InputType::{LnUrlAuth, LnUrlPay, LnUrlWithdraw};
use breez_sdk_core::{
    parse, BreezEvent, BreezServices, BuyBitcoinRequest, CheckMessageRequest, EventListener,
    GreenlightCredentials, ListPaymentsRequest, LnUrlPayRequest, LnUrlWithdrawRequest,
    PrepareRefundRequest, ReceiveOnchainRequest, ReceivePaymentRequest, RefundRequest,
    ReportIssueRequest, ReportPaymentFailureDetails, ReverseSwapFeesRequest, SendOnchainRequest,
    SendPaymentRequest, SendSpontaneousPaymentRequest, SignMessageRequest, StaticBackupRequest,
    SweepRequest,
};
use breez_sdk_core::{Config, GreenlightNodeConfig, NodeConfig};
use once_cell::sync::OnceCell;
use qrcode_rs::render::unicode;
use qrcode_rs::{EcLevel, QrCode};
use rustyline::history::DefaultHistory;

use crate::persist::CliPersistence;
use crate::Commands;

use std::borrow::Cow::{self, Owned};

use rustyline::highlight::Highlighter;
use rustyline::hint::HistoryHinter;
use rustyline::Editor;
use rustyline::{Completer, Helper, Hinter, Validator};

static BREEZ_SERVICES: OnceCell<Arc<BreezServices>> = OnceCell::new();

fn sdk() -> Result<Arc<BreezServices>> {
    BREEZ_SERVICES
        .get()
        .ok_or("Breez Services not initialized")
        .map_err(|err| anyhow!(err))
        .cloned()
}

struct CliEventListener {}

impl EventListener for CliEventListener {
    fn on_event(&self, e: BreezEvent) {
        info!("Received Breez event: {:?}", e);
    }
}

async fn connect(config: Config, seed: &[u8]) -> Result<()> {
    let service =
        BreezServices::connect(config, seed.to_vec(), Box::new(CliEventListener {})).await?;
    BREEZ_SERVICES
        .set(service)
        .map_err(|_| anyhow!("Breez Services already initialized"))?;

    Ok(())
}

#[derive(Helper, Completer, Hinter, Validator)]
pub(crate) struct CliHelper {
    #[rustyline(Hinter)]
    pub(crate) hinter: HistoryHinter,
}

impl Highlighter for CliHelper {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Owned("\x1b[1m".to_owned() + hint + "\x1b[m")
    }
}

pub(crate) async fn handle_command(
    rl: &mut Editor<CliHelper, DefaultHistory>,
    persistence: &CliPersistence,
    command: Commands,
) -> Result<String, Error> {
    match command {
        Commands::SetAPIKey { key } => {
            let mut config = persistence.get_or_create_config()?;
            config.api_key = Some(key);
            persistence.save_config(config)?;
            Ok("API key was set".to_string())
        }
        Commands::SetEnv { env } => {
            let mut config = persistence.get_or_create_config()?;
            config.env = env.clone();
            persistence.save_config(config)?;
            Ok(format!("Environment was set to {:?}", env))
        }
        Commands::Connect {
            partner_cert,
            partner_key,
            invite_code,
        } => {
            let mut config = persistence
                .get_or_create_config()?
                .to_sdk_config(&persistence.data_dir);
            let mut partner_credentials: Option<GreenlightCredentials> = None;
            if partner_cert.is_some() && partner_key.is_some() {
                let cert = fs::read(partner_cert.unwrap())?;
                let key = fs::read(partner_key.unwrap())?;
                partner_credentials = Some(GreenlightCredentials {
                    device_cert: cert,
                    device_key: key,
                })
            }

            config.node_config = NodeConfig::Greenlight {
                config: GreenlightNodeConfig {
                    partner_credentials,
                    invite_code,
                },
            };

            connect(config, &persistence.get_or_create_seed()).await?;
            Ok("Node was registered succesfully".to_string())
        }
        Commands::Sync {} => {
            sdk()?.sync().await?;
            Ok("Sync finished successfully".to_string())
        }
        Commands::Parse { input } => parse(&input)
            .await
            .map(|res| serde_json::to_string_pretty(&res))?
            .map_err(|e| e.into()),
        Commands::ReceivePayment {
            amount_msat,
            description,
            use_description_hash,
            expiry,
            cltv,
        } => {
            let recv_payment_response = sdk()?
                .receive_payment(ReceivePaymentRequest {
                    amount_msat,
                    description,
                    use_description_hash,
                    expiry,
                    cltv,
                    ..Default::default()
                })
                .await?;
            let mut result = serde_json::to_string(&recv_payment_response)?;
            result.push('\n');
            result.push_str(&build_qr_text(&recv_payment_response.ln_invoice.bolt11));
            Ok(result)
        }
        Commands::SendOnchain {
            amount_sat,
            onchain_recipient_address,
            sat_per_vbyte,
        } => {
            let pair_info = sdk()?
                .fetch_reverse_swap_fees(ReverseSwapFeesRequest::default())
                .await
                .map_err(|e| anyhow!("Failed to fetch reverse swap fee infos: {e}"))?;

            let rev_swap_res = sdk()?
                .send_onchain(SendOnchainRequest {
                    amount_sat,
                    onchain_recipient_address,
                    pair_hash: pair_info.fees_hash,
                    sat_per_vbyte,
                })
                .await?;
            serde_json::to_string_pretty(&rev_swap_res.reverse_swap_info).map_err(|e| e.into())
        }
        Commands::MaxReverseSwapAmount {} => {
            let response = sdk()?.max_reverse_swap_amount().await?;
            serde_json::to_string_pretty(&response).map_err(|e| e.into())
        }
        Commands::FetchOnchainFees { send_amount_sat } => {
            let pair_info = sdk()?
                .fetch_reverse_swap_fees(ReverseSwapFeesRequest { send_amount_sat })
                .await
                .map_err(|e| anyhow!("Failed to fetch reverse swap fee infos: {e}"))?;
            serde_json::to_string_pretty(&pair_info).map_err(|e| e.into())
        }
        Commands::InProgressReverseSwaps {} => {
            let mut res: Vec<String> = vec![];
            for rsi in &sdk()?.in_progress_reverse_swaps().await? {
                res.push(format!(
                    "Reverse swap {} is in progress with status {:?}",
                    rsi.id, rsi.status
                ));
            }
            serde_json::to_string_pretty(&res).map_err(|e| e.into())
        }
        Commands::SendPayment {
            bolt11,
            amount_msat,
        } => {
            let payment = sdk()?
                .send_payment(SendPaymentRequest {
                    bolt11,
                    amount_msat,
                })
                .await?;
            serde_json::to_string_pretty(&payment).map_err(|e| e.into())
        }
        Commands::SendSpontaneousPayment {
            node_id,
            amount_msat,
        } => {
            let response = sdk()?
                .send_spontaneous_payment(SendSpontaneousPaymentRequest {
                    node_id,
                    amount_msat,
                    extra_tlvs: None,
                })
                .await?;
            serde_json::to_string_pretty(&response.payment).map_err(|e| e.into())
        }
        Commands::ListPayments {
            from_timestamp,
            to_timestamp,
            include_failures,
            limit,
            offset,
        } => {
            let payments = sdk()?
                .list_payments(ListPaymentsRequest {
                    filters: None,
                    from_timestamp,
                    to_timestamp,
                    include_failures: Some(include_failures),
                    limit,
                    offset,
                })
                .await?;
            serde_json::to_string_pretty(&payments).map_err(|e| e.into())
        }
        Commands::PaymentByHash { hash } => {
            let payment = sdk()?.payment_by_hash(hash).await?;
            serde_json::to_string_pretty(&payment).map_err(|e| e.into())
        }
        Commands::Sweep {
            to_address,
            sat_per_vbyte,
        } => {
            sdk()?
                .sweep(SweepRequest {
                    to_address,
                    sat_per_vbyte,
                })
                .await?;
            Ok("Onchain funds were swept successfully".to_string())
        }
        Commands::PrepareSweep {
            to_address,
            sat_per_vbyte,
        } => {
            sdk()?
                .sweep(SweepRequest {
                    to_address,
                    sat_per_vbyte,
                })
                .await?;
            Ok("Onchain funds were swept succesfully".to_string())
        }
        Commands::ListLsps {} => {
            let lsps = sdk()?.list_lsps().await?;
            serde_json::to_string_pretty(&lsps).map_err(|e| e.into())
        }
        Commands::LspInfo {} => {
            let lsp_info = sdk()?.lsp_info().await?;
            serde_json::to_string_pretty(&lsp_info).map_err(|e| e.into())
        }
        Commands::ConnectLSP { lsp_id } => {
            sdk()?.connect_lsp(lsp_id).await?;
            Ok("LSP connected successfully".to_string())
        }
        Commands::OpenChannelFee {
            amount_msat,
            expiry,
        } => {
            let res = sdk()?
                .open_channel_fee(breez_sdk_core::OpenChannelFeeRequest {
                    amount_msat,
                    expiry,
                })
                .await?;
            serde_json::to_string_pretty(&res).map_err(|e| e.into())
        }
        Commands::NodeCredentials {} => match sdk()?.node_credentials()? {
            Some(credentials) => serde_json::to_string_pretty(&credentials).map_err(|e| e.into()),
            None => Ok("No credentials".into()),
        },
        Commands::NodeInfo {} => {
            serde_json::to_string_pretty(&sdk()?.node_info()?).map_err(|e| e.into())
        }
        Commands::ListFiat {} => {
            serde_json::to_string_pretty(&sdk()?.list_fiat_currencies().await?)
                .map_err(|e| e.into())
        }
        Commands::FetchFiatRates {} => {
            serde_json::to_string_pretty(&sdk()?.fetch_fiat_rates().await?).map_err(|e| e.into())
        }
        Commands::CloseLSPChannels {} => {
            let tx_ids = sdk()?.close_lsp_channels().await?;
            Ok(format!("Closing transaction ids:\n{:?}", tx_ids))
        }
        Commands::Disconnect {} => {
            sdk()?.disconnect().await?;
            Ok("Node was stopped successfully".to_string())
        }
        Commands::RecommendedFees {} => {
            serde_json::to_string_pretty(&sdk()?.recommended_fees().await?).map_err(|e| e.into())
        }
        Commands::ReceiveOnchain {} => serde_json::to_string_pretty(
            &sdk()?
                .receive_onchain(ReceiveOnchainRequest::default())
                .await?,
        )
        .map_err(|e| e.into()),
        Commands::InProgressSwap {} => {
            serde_json::to_string_pretty(&sdk()?.in_progress_swap().await?).map_err(|e| e.into())
        }
        Commands::ListRefundables {} => {
            serde_json::to_string_pretty(&sdk()?.list_refundables().await?).map_err(|e| e.into())
        }
        Commands::PrepareRefund {
            swap_address,
            to_address,
            sat_per_vbyte,
        } => {
            let res = sdk()?
                .prepare_refund(PrepareRefundRequest {
                    swap_address,
                    to_address,
                    sat_per_vbyte,
                })
                .await?;
            Ok(format!(
                "Prepared refund tx - weight: {} - fees: {} sat",
                res.refund_tx_weight, res.refund_tx_fee_sat
            ))
        }
        Commands::Refund {
            swap_address,
            to_address,
            sat_per_vbyte,
        } => {
            let res = sdk()?
                .refund(RefundRequest {
                    swap_address,
                    to_address,
                    sat_per_vbyte,
                })
                .await?;
            Ok(format!("Refund tx: {}", res.refund_tx_id))
        }
        Commands::SignMessage { message } => {
            let req = SignMessageRequest { message };
            let res = sdk()?.sign_message(req).await?;
            Ok(format!("Message signature: {}", res.signature))
        }
        Commands::CheckMessage {
            message,
            pubkey,
            signature,
        } => {
            let req = CheckMessageRequest {
                message,
                pubkey,
                signature,
            };
            let res = sdk()?.check_message(req).await?;
            Ok(format!("Message was signed by node: {}", res.is_valid))
        }
        Commands::LnurlPay { lnurl } => match parse(&lnurl).await? {
            LnUrlPay { data: pd } => {
                let prompt = format!(
                    "Amount to pay in millisatoshi (min {} msat, max {} msat: ",
                    pd.min_sendable, pd.max_sendable
                );

                let amount_msat = rl.readline(&prompt)?;
                let pay_res = sdk()?
                    .lnurl_pay(LnUrlPayRequest {
                        data: pd,
                        amount_msat: amount_msat.parse::<u64>()?,
                        comment: None,
                    })
                    .await?;
                //show_results(pay_res);
                serde_json::to_string_pretty(&pay_res).map_err(|e| e.into())
            }
            _ => Err(anyhow!("Invalid input")),
        },
        Commands::LnurlWithdraw { lnurl } => {
            match parse(&lnurl).await? {
                LnUrlWithdraw { data: wd } => {
                    info!("Endpoint description: {}", wd.default_description);

                    // Bounds for a withdrawal amount. Normally these would also consider NodeState params:
                    // max can receive = min(maxWithdrawable, local estimation of how much can be routed into wallet)
                    // min can receive = max(minWithdrawable, local minimal value allowed by wallet)
                    // However, for simplicity, we just use the LNURL-withdraw min/max bounds
                    let user_input_max_msat = wd.max_withdrawable;
                    let user_input_min_msat = 2_001_000;

                    if user_input_max_msat < user_input_min_msat {
                        error!("The LNURLw endpoint needs to accept at least {} msat, but min / max withdrawable are {} msat / {} msat",
                            user_input_min_msat,
                            wd.min_withdrawable,
                            wd.max_withdrawable
                        );
                        return Ok("".to_string());
                    }

                    let prompt = format!(
                        "Amount to withdraw in msat (min {} msat, max {} msat: ",
                        user_input_min_msat, user_input_max_msat,
                    );
                    let user_input_withdraw_amount_msat = rl.readline(&prompt)?;

                    let amount_msat: u64 = user_input_withdraw_amount_msat.parse()?;
                    let description = "LNURL-withdraw";

                    let withdraw_res = sdk()?
                        .lnurl_withdraw(LnUrlWithdrawRequest {
                            data: wd,
                            amount_msat,
                            description: Some(description.into()),
                        })
                        .await?;
                    serde_json::to_string_pretty(&withdraw_res).map_err(|e| e.into())
                }
                _ => Err(anyhow!("Unexpected result type")),
            }
        }
        Commands::LnurlAuth { lnurl } => {
            let lnurl_endpoint = lnurl.trim();

            match parse(lnurl_endpoint).await? {
                LnUrlAuth { data: ad } => {
                    let auth_res = sdk()?.lnurl_auth(ad).await?;
                    serde_json::to_string_pretty(&auth_res).map_err(|e| e.into())
                }
                _ => Err(anyhow!("Unexpected result type")),
            }
        }
        Commands::ServiceHealthCheck {} => {
            let health_check = sdk()?.service_health_check().await?;
            serde_json::to_string_pretty(&health_check).map_err(|e| e.into())
        }
        Commands::ReportPaymentFailure {
            payment_hash,
            comment,
        } => {
            sdk()?
                .report_issue(ReportIssueRequest::PaymentFailure {
                    data: ReportPaymentFailureDetails {
                        payment_hash,
                        comment,
                    },
                })
                .await?;
            Ok("Report sent".into())
        }
        Commands::ExecuteDevCommand { command } => {
            serde_json::to_string_pretty(&sdk()?.execute_dev_command(command).await?)
                .map_err(|e| e.into())
        }
        Commands::BuyBitcoin { provider } => {
            let res = sdk()?
                .buy_bitcoin(BuyBitcoinRequest {
                    provider: provider.clone(),
                    opening_fee_params: None,
                })
                .await?;
            Ok(format!("Here your {:?} url: {}", provider, res.url))
        }
        Commands::Backup {} => {
            sdk().unwrap().backup().await?;
            Ok("Backup completed successfully".into())
        }
        Commands::StaticBackup {} => {
            let config = persistence
                .get_or_create_config()?
                .to_sdk_config(&persistence.data_dir);
            let backup_data = BreezServices::static_backup(StaticBackupRequest {
                working_dir: config.working_dir,
            })?;
            match backup_data.backup {
                Some(backup) => {
                    let backup_str = serde_json::to_string_pretty(&backup)?;
                    Ok(format!("Static backup data:\n{}", backup_str))
                }
                None => Ok("No static backup data".into()),
            }
        }
    }
}

fn build_qr_text(text: &str) -> String {
    QrCode::with_error_correction_level(text, EcLevel::L)
        .unwrap()
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .build()
}
