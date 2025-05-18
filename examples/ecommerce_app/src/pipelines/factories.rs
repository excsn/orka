// orka_project/examples/ecommerce_app/src/pipelines/factories.rs
use crate::errors::AppError; // Application's error type
use crate::pipelines::contexts::{CheckoutCtxData, MockPaymentProviderSubCtxData};
use crate::services::payment_mock;
// OrkaResult is Result<_, OrkaError>.
// The factory output needs to be Result<Arc<Pipeline<SData, AppError>>, OrkaError>
use orka::{ContextData, OrkaError, Pipeline, PipelineControl};
use std::future::Future;
use std::sync::Arc;
use tracing::{error, info, instrument, Level};

// Factory for Mock Provider A (simulating Stripe)
// The factory's future must resolve to Result<Arc<Pipeline<SData, AppError>>, OrkaError>.
#[instrument(
    name = "factory::mock_provider_a",
    skip(main_checkout_ctx_data),
    fields(
        main_ctx_order_id = ?main_checkout_ctx_data.read().order_id,
        target_sdata_type = %std::any::type_name::<MockPaymentProviderSubCtxData>(),
        target_pipeline_error_type = %std::any::type_name::<AppError>()
    ),
    err(Display) // This will display the OrkaError if the factory itself fails
)]
pub fn mock_provider_a_pipeline_factory(
  main_checkout_ctx_data: ContextData<CheckoutCtxData>,
) -> impl Future<Output = Result<Arc<Pipeline<MockPaymentProviderSubCtxData, AppError>>, OrkaError>> + Send {
  let (order_id_val, account_id_val_from_main_ctx) = {
    let guard = main_checkout_ctx_data.read();
    let order_id = guard.order_id.ok_or_else(|| {
      // Factory setup error: should return OrkaError
      error!("Factory (Mock A): Order ID missing from main context during factory setup.");
      OrkaError::ConfigurationError {
        step_name: "mock_provider_a_pipeline_factory_setup".to_string(),
        message: "Order ID missing from main context".to_string(),
      }
    });
    // Handle potential error from reading order_id (though ok_or_else should be caught by compiler if not Result)
    // For this example, let's assume order_id is present or we'd return Err(OrkaError) from the factory sync part.
    // A more robust factory might look like:
    // if guard.order_id.is_none() {
    //     return future::ready(Err(OrkaError::ConfigurationError {...})); // if not async
    // }
    let order_id_unwrapped = guard
      .order_id
      .expect("Order ID must be set for factory A (checked by logic or type system)");

    let account_id = guard
      .current_payment_account_id_for_sub_ctx_init
      .clone()
      .unwrap_or_else(|| guard.app_state.config.mock_payment_provider_main_id.clone());
    (order_id_unwrapped, account_id)
  };

  async move {
    info!(
      "Factory (Mock A): Creating pipeline <SData, AppError> for order {}, account {}",
      order_id_val, account_id_val_from_main_ctx
    );

    // If factory logic itself can fail here (e.g. loading a dynamic config specific to this pipeline)
    // it should return Err(OrkaError).
    // Example:
    // if some_condition_for_factory_failure {
    //     return Err(OrkaError::Internal("Factory A failed to initialize pipeline components".to_string()));
    // }

    // Pipeline is over SData (MockPaymentProviderSubCtxData) and its handlers use AppError
    let mut p = Pipeline::<MockPaymentProviderSubCtxData, AppError>::new(&[
      ("create_intent_mock_a", false, None),
      ("confirm_payment_mock_a", false, None),
    ]);

    // Handler takes ContextData<MockPaymentProviderSubCtxData> and returns Result<PipelineControl, AppError>
    p.on_root(
      "create_intent_mock_a",
      move |sub_ctx_data: ContextData<MockPaymentProviderSubCtxData>| {
        Box::pin(async move {
          // This async block returns Result<PipelineControl, AppError>
          let (o_id, amt, curr, acc_id) = {
            let guard = sub_ctx_data.read();
            (
              guard.order_id,
              guard.amount_cents,
              guard.currency.clone(),
              guard.using_account_id.clone(),
            )
          };
          info!(
            "Scoped Pipeline (Mock A - Order {}): Creating payment intent with account {}",
            o_id, acc_id
          );

          // payment_mock::create_mock_payment_intent returns Result<_, AppError>
          // `?` propagates AppError directly.
          let intent = payment_mock::create_mock_payment_intent(o_id, amt, &curr, &acc_id).await?;

          {
            let mut guard = sub_ctx_data.write();
            guard.payment_intent = Some(intent);
            guard.succeeded = false;
          }
          info!("Scoped Pipeline (Mock A - Order {}): Payment intent created.", o_id);
          Ok::<_, AppError>(PipelineControl::Continue)
        })
      },
    );

    p.on_root(
      "confirm_payment_mock_a",
      |sub_ctx_data: ContextData<MockPaymentProviderSubCtxData>| {
        Box::pin(async move {
          // This async block returns Result<PipelineControl, AppError>
          let mut intent_to_confirm = {
            let guard = sub_ctx_data.read();
            guard.payment_intent.clone().ok_or_else(|| {
              // This is an internal logic error within this sub-pipeline, return AppError.
              let err_msg = "Mock intent not found for confirmation in Mock A pipeline.";
              error!(step_name = "confirm_payment_mock_a", %err_msg);
              AppError::Internal(err_msg.to_string())
            })?
          };
          let order_id_log = intent_to_confirm.id.clone();
          info!(
            "Scoped Pipeline (Mock A - Intent {}): Confirming payment.",
            order_id_log
          );

          // payment_mock::confirm_mock_payment returns Result<(), AppError>
          // `?` propagates AppError directly.
          payment_mock::confirm_mock_payment(&mut intent_to_confirm).await?;

          {
            let mut guard = sub_ctx_data.write();
            guard.payment_intent = Some(intent_to_confirm.clone());
            guard.succeeded = intent_to_confirm.status == "succeeded";
            if !guard.succeeded {
              info!(
                "Scoped Pipeline (Mock A - Intent {}): Payment confirmation FAILED. Stopping scoped pipeline.",
                order_id_log
              );
              return Ok::<_, AppError>(PipelineControl::Stop); // Still Ok Result, but Stop control.
            }
          }
          info!(
            "Scoped Pipeline (Mock A - Intent {}): Payment confirmed SUCCESSFULLY.",
            order_id_log
          );
          Ok(PipelineControl::Continue)
        })
      },
    );
    Ok(Arc::new(p)) // Factory returns Ok(Arc<Pipeline<SData, AppError>>)
  }
}

// Factory for Mock Provider B (simulating PayPal)
// The factory's future must resolve to Result<Arc<Pipeline<SData, AppError>>, OrkaError>.
#[instrument(
    name = "factory::mock_provider_b",
    skip(main_checkout_ctx_data),
    fields(
        main_ctx_order_id = ?main_checkout_ctx_data.read().order_id,
        target_sdata_type = %std::any::type_name::<MockPaymentProviderSubCtxData>(),
        target_pipeline_error_type = %std::any::type_name::<AppError>()
    ),
    err(Display) // This will display the OrkaError if the factory itself fails
)]
pub fn mock_provider_b_pipeline_factory(
  main_checkout_ctx_data: ContextData<CheckoutCtxData>,
) -> impl Future<Output = Result<Arc<Pipeline<MockPaymentProviderSubCtxData, AppError>>, OrkaError>> + Send {
  let (order_id_val, account_id_val_from_main_ctx) = {
    let guard = main_checkout_ctx_data.read();
    let order_id_unwrapped = guard.order_id.expect("Order ID must be set for factory B");
    let account_id = guard
      .current_payment_account_id_for_sub_ctx_init
      .clone()
      .unwrap_or_else(|| guard.app_state.config.mock_payment_provider_alt_id.clone());
    (order_id_unwrapped, account_id)
  };

  async move {
    info!(
      "Factory (Mock B): Creating pipeline <SData, AppError> for order {}, account {}",
      order_id_val, account_id_val_from_main_ctx
    );
    let mut p = Pipeline::<MockPaymentProviderSubCtxData, AppError>::new(&[
      ("create_order_mock_b", false, None),
      ("capture_funds_mock_b", false, None),
    ]);

    p.on_root(
      "create_order_mock_b",
      move |sub_ctx_data: ContextData<MockPaymentProviderSubCtxData>| {
        Box::pin(async move {
          // Returns Result<PipelineControl, AppError>
          let (o_id, amt, curr, acc_id) = {
            let guard = sub_ctx_data.read();
            (
              guard.order_id,
              guard.amount_cents,
              guard.currency.clone(),
              guard.using_account_id.clone(),
            )
          };
          info!(
            "Scoped Pipeline (Mock B - Order {}): Creating order with account {}.",
            o_id, acc_id
          );
          let mock_order_details = payment_mock::MockPaymentIntent {
            id: format!("mock_b_ord_{}", uuid::Uuid::new_v4().simple()),
            amount: amt,
            currency: curr,
            status: "created".to_string(),
            client_secret: None,
            account_id_used: acc_id,
          };
          {
            let mut guard = sub_ctx_data.write();
            guard.payment_intent = Some(mock_order_details);
            guard.succeeded = false;
          }
          info!(
            "Scoped Pipeline (Mock B - Order {}): Mock order created in sub-context.",
            o_id
          );
          Ok::<_, AppError>(PipelineControl::Continue)
        })
      },
    );

    p.on_root(
      "capture_funds_mock_b",
      |sub_ctx_data: ContextData<MockPaymentProviderSubCtxData>| {
        Box::pin(async move {
          // Returns Result<PipelineControl, AppError>
          let (order_id_log, mut mock_order_to_capture) = {
            let guard = sub_ctx_data.read();
            (
              guard.order_id,
              guard
                .payment_intent
                .clone()
                .ok_or_else(|| AppError::Internal("Mock B order details not found for capture.".to_string()))?,
            )
          };
          info!(
            "Scoped Pipeline (Mock B - Order {}): Capturing funds for mock order ID {}",
            order_id_log, mock_order_to_capture.id
          );
          mock_order_to_capture.status = "succeeded".to_string();
          {
            let mut guard = sub_ctx_data.write();
            guard.payment_intent = Some(mock_order_to_capture.clone());
            guard.succeeded = mock_order_to_capture.status == "succeeded";
            if !guard.succeeded {
              info!(
                "Scoped Pipeline (Mock B - Order {}): Funds capture FAILED. Stopping scoped pipeline.",
                order_id_log
              );
              return Ok::<_, AppError>(PipelineControl::Stop);
            }
          }
          info!(
            "Scoped Pipeline (Mock B - Order {}): Funds captured SUCCESSFULLY.",
            order_id_log
          );
          Ok(PipelineControl::Continue)
        })
      },
    );
    Ok(Arc::new(p)) // Factory returns Ok(Arc<Pipeline<SData, AppError>>)
  }
}
