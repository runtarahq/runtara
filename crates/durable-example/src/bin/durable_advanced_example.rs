// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Order Processing Workflow - Durable Execution Example
//!
//! Simulates a complete order fulfillment pipeline:
//! 1. Validate order
//! 2. Reserve inventory
//! 3. Capture payment
//! 4. Create shipment
//! 5. Send notifications
//!
//! Each step is durable - if the process crashes, it resumes from the last checkpoint.
//!
//! Run with: cargo run -p durable-example --bin durable_advanced_example

use runtara_sdk::{RuntaraSdk, durable};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ============================================================================
// Domain Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub customer_email: String,
    pub items: Vec<OrderItem>,
    pub total_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderItem {
    pub sku: String,
    pub quantity: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessedOrder {
    pub order_id: String,
    pub payment_id: String,
    pub shipment_id: String,
    pub tracking_number: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowError {
    pub step: String,
    pub message: String,
}

impl std::fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.step, self.message)
    }
}
impl std::error::Error for WorkflowError {}

// ============================================================================
// Simulated External Services
// ============================================================================

#[derive(Clone)]
pub struct PaymentGateway;

impl PaymentGateway {
    pub async fn capture(&self, order_id: &str, amount_cents: u64) -> Result<String, String> {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Ok(format!("pay_{}_{}c", order_id, amount_cents))
    }
}

#[derive(Clone)]
pub struct InventoryService;

impl InventoryService {
    pub async fn reserve(&self, sku: &str, qty: u32) -> Result<String, String> {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        Ok(format!("res_{}_{}", sku, qty))
    }
}

#[derive(Clone)]
pub struct ShippingService;

impl ShippingService {
    pub async fn create_shipment(&self, order_id: &str) -> Result<(String, String), String> {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let shipment_id = format!("ship_{}", order_id);
        let tracking = format!("TRK{}", uuid::Uuid::new_v4().simple());
        Ok((shipment_id, tracking))
    }
}

#[derive(Clone)]
pub struct NotificationService;

impl NotificationService {
    pub async fn send_email(&self, to: &str, subject: &str, _body: &str) -> Result<(), String> {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        println!("  [Email] To: {} | Subject: {}", to, subject);
        Ok(())
    }
}

// ============================================================================
// Durable Workflow Steps
// ============================================================================

/// Step 1: Validate the order
#[durable]
pub async fn validate_order(key: &str, order: &Order) -> Result<(), WorkflowError> {
    if order.items.is_empty() {
        return Err(WorkflowError {
            step: "validation".into(),
            message: "Order has no items".into(),
        });
    }
    if order.total_cents == 0 {
        return Err(WorkflowError {
            step: "validation".into(),
            message: "Order total is zero".into(),
        });
    }
    println!("  [✓] Order validated");
    Ok(())
}

/// Step 2: Reserve inventory for each item
#[durable]
pub async fn reserve_inventory(
    key: &str,
    inventory: Arc<InventoryService>,
    items: &[OrderItem],
) -> Result<Vec<String>, WorkflowError> {
    let mut reservations = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let res_key = format!("{}-item-{}", key, i);
        let res_id =
            reserve_single_item(&res_key, inventory.clone(), &item.sku, item.quantity).await?;
        reservations.push(res_id);
    }
    println!("  [✓] Inventory reserved: {} items", reservations.len());
    Ok(reservations)
}

#[durable]
async fn reserve_single_item(
    key: &str,
    inventory: Arc<InventoryService>,
    sku: &str,
    qty: u32,
) -> Result<String, WorkflowError> {
    inventory
        .reserve(sku, qty)
        .await
        .map_err(|e| WorkflowError {
            step: "inventory".into(),
            message: e,
        })
}

/// Step 3: Capture payment
#[durable]
pub async fn capture_payment(
    key: &str,
    gateway: Arc<PaymentGateway>,
    order_id: &str,
    amount_cents: u64,
) -> Result<String, WorkflowError> {
    let payment_id = gateway
        .capture(order_id, amount_cents)
        .await
        .map_err(|e| WorkflowError {
            step: "payment".into(),
            message: e,
        })?;
    println!("  [✓] Payment captured: {}", payment_id);
    Ok(payment_id)
}

/// Step 4: Create shipment
#[durable]
pub async fn create_shipment(
    key: &str,
    shipping: Arc<ShippingService>,
    order_id: &str,
) -> Result<(String, String), WorkflowError> {
    let (shipment_id, tracking) =
        shipping
            .create_shipment(order_id)
            .await
            .map_err(|e| WorkflowError {
                step: "shipping".into(),
                message: e,
            })?;
    println!("  [✓] Shipment created: {} ({})", shipment_id, tracking);
    Ok((shipment_id, tracking))
}

/// Step 5: Send notifications
#[durable]
pub async fn send_notifications(
    key: &str,
    notifications: Arc<NotificationService>,
    email: &str,
    order_id: &str,
    tracking: &str,
) -> Result<(), WorkflowError> {
    // Confirmation email
    notifications
        .send_email(
            email,
            &format!("Order {} confirmed", order_id),
            "Thank you for your order!",
        )
        .await
        .map_err(|e| WorkflowError {
            step: "notification".into(),
            message: e,
        })?;

    // Shipping email
    notifications
        .send_email(
            email,
            &format!("Order {} shipped", order_id),
            &format!("Track your order: {}", tracking),
        )
        .await
        .map_err(|e| WorkflowError {
            step: "notification".into(),
            message: e,
        })?;

    println!("  [✓] Notifications sent");
    Ok(())
}

/// Main workflow: orchestrates all steps
#[durable]
pub async fn process_order(
    key: &str,
    order: &Order,
    payment: Arc<PaymentGateway>,
    inventory: Arc<InventoryService>,
    shipping: Arc<ShippingService>,
    notifications: Arc<NotificationService>,
) -> Result<ProcessedOrder, WorkflowError> {
    println!("Processing order: {}", order.id);

    // Each step is independently cached
    validate_order(&format!("{}-validate", key), order).await?;

    reserve_inventory(&format!("{}-inventory", key), inventory, &order.items).await?;

    let payment_id = capture_payment(
        &format!("{}-payment", key),
        payment,
        &order.id,
        order.total_cents,
    )
    .await?;

    let (shipment_id, tracking) =
        create_shipment(&format!("{}-shipping", key), shipping, &order.id).await?;

    send_notifications(
        &format!("{}-notify", key),
        notifications,
        &order.customer_email,
        &order.id,
        &tracking,
    )
    .await?;

    println!("Order {} completed!\n", order.id);

    Ok(ProcessedOrder {
        order_id: order.id.clone(),
        payment_id,
        shipment_id,
        tracking_number: tracking,
    })
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let instance_id = format!("order-workflow-{}", uuid::Uuid::new_v4());
    RuntaraSdk::localhost(&instance_id, "demo-tenant")?
        .init(None)
        .await?;

    // Initialize services
    let payment = Arc::new(PaymentGateway);
    let inventory = Arc::new(InventoryService);
    let shipping = Arc::new(ShippingService);
    let notifications = Arc::new(NotificationService);

    // Process first order
    let order1 = Order {
        id: "ORD-001".into(),
        customer_email: "alice@example.com".into(),
        items: vec![
            OrderItem {
                sku: "WIDGET-A".into(),
                quantity: 2,
            },
            OrderItem {
                sku: "GADGET-B".into(),
                quantity: 1,
            },
        ],
        total_cents: 4999,
    };

    println!("\n=== First run ===");
    let result = process_order(
        "order-001",
        &order1,
        payment.clone(),
        inventory.clone(),
        shipping.clone(),
        notifications.clone(),
    )
    .await?;
    println!("Result: {:?}", result);

    // Second call with same key - entire workflow returns cached result
    println!("\n=== Second run (cached) ===");
    let result2 = process_order(
        "order-001",
        &order1,
        payment.clone(),
        inventory.clone(),
        shipping.clone(),
        notifications.clone(),
    )
    .await?;
    println!("Result: {:?}", result2);

    // Different order with different key
    println!("\n=== Different order ===");
    let order2 = Order {
        id: "ORD-002".into(),
        customer_email: "bob@example.com".into(),
        items: vec![OrderItem {
            sku: "PREMIUM-X".into(),
            quantity: 1,
        }],
        total_cents: 9999,
    };

    let result3 = process_order(
        "order-002",
        &order2,
        payment,
        inventory,
        shipping,
        notifications,
    )
    .await?;
    println!("Result: {:?}", result3);

    Ok(())
}
