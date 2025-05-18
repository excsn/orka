# Orka E-commerce API Example

This example demonstrates a simplified backend for an e-commerce application built using Rust, Actix Web, SQLx (with PostgreSQL), and the **Orka Workflow Engine**.

It showcases how Orka can be used to orchestrate complex business processes like user signup, signin, adding items to a cart, a multi-step checkout process with conditional payment provider logic, and handling incoming webhooks.

## Features

*   **User Authentication:**
    *   Signup with email and password (password hashing with Argon2).
    *   Signin with email and password.
    *   (Mock) Session token issuance.
*   **Product Catalog:**
    *   List products.
    *   Get product details.
*   **Shopping Cart:**
    *   Add items to a user's cart (validates product existence and stock).
*   **Checkout Process (Orka Pipeline):**
    *   Creates an initial order record.
    *   Fetches user details for confirmation.
    *   **Conditionally routes to different (mock) payment provider sub-pipelines based on order details.**
    *   Updates order status post-payment.
    *   Sends a (mock) order confirmation email.
*   **Webhook Handling (Orka Pipeline):**
    *   Generic endpoint for receiving webhooks (e.g., from payment gateways).
    *   (Mock) Signature verification.
    *   Payload parsing.
    *   **Conditionally routes events to specific event processing sub-pipelines.**
*   **Error Handling:** Robust application-level error handling integrated with Actix Web.
*   **Configuration:** Loads settings from environment variables.
*   **Logging:** Uses `tracing` for structured logging.

## Core Technologies

*   **Rust:** The programming language.
*   **Orka Workflow Engine:** For orchestrating multi-step business processes.
*   **Actix Web:** High-performance web framework for building the API.
*   **SQLx:** Asynchronous SQL toolkit for PostgreSQL interaction (using runtime query preparation for easier demo setup).
*   **Tokio:** Asynchronous runtime.
*   **Serde:** For serialization and deserialization (JSON).
*   **uuid:** For generating and using UUIDs.
*   **chrono:** For date and time handling.
*   **Argon2:** For password hashing.
*   **tracing:** For application logging.
*   **thiserror, anyhow:** For error handling.
*   **dotenvy:** For loading environment variables from a `.env` file.

## Project Structure

```
examples/ecommerce_app/
├── Cargo.toml
├── .env.example       # Example environment variables
└── src/
    ├── main.rs            # Application entry point, server setup
    ├── config.rs          # Configuration loading
    ├── errors.rs          # Custom error types
    ├── state.rs           # Shared application state
    ├── models/            # Database entity structs
    ├── services/          # Business logic services (mocks)
    ├── pipelines/         # Orka pipeline definitions
    │   ├── contexts.rs      # Data contexts for pipelines
    │   ├── factories.rs     # Factories for dynamic scoped pipelines
    │   ├── common_steps.rs  # Reusable pipeline steps
    │   ├── *.rs             # Specific pipeline definitions
    └── web/               # Actix Web handlers and routing
        ├── handlers/        # Request handler modules
        └── routes.rs        # Route configuration
```

## Setup and Running

### Prerequisites

1.  **Rust:** Install the latest stable version of Rust (see [rustup.rs](https://rustup.rs/)).
2.  **PostgreSQL:** A running PostgreSQL instance. You can use Docker, a local installation, or a cloud-hosted instance.
3.  **(Optional) `sqlx-cli`:** If you want to manage migrations or use compile-time checked queries later (this example is configured for runtime queries to simplify initial setup). Install with `cargo install sqlx-cli`.

### Database Setup

1.  **Create a PostgreSQL database and user.**
    For example, using `psql`:
    ```sql
    CREATE DATABASE orka_ecommerce_db;
    CREATE USER orka_user WITH ENCRYPTED PASSWORD 'your_strong_password';
    GRANT ALL PRIVILEGES ON DATABASE orka_ecommerce_db TO orka_user;
    ```
    *Note: For a production setup, grant more restrictive permissions.*

2.  **Create Tables:**
    You'll need to create the necessary tables. A sample `schema.sql` (you would create this file) might look like this:

    ```sql
    -- examples/ecommerce_app/schema.sql (Create this file)

    CREATE EXTENSION IF NOT EXISTS "uuid-ossp"; -- For uuid_generate_v4()

    CREATE TABLE users (
        id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
        email TEXT UNIQUE NOT NULL,
        password_hash TEXT NOT NULL,
        -- name TEXT, -- Optional
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    );

    CREATE TABLE products (
        id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
        name TEXT NOT NULL,
        description TEXT,
        price_cents INT NOT NULL CHECK (price_cents >= 0),
        stock_quantity INT NOT NULL DEFAULT 0 CHECK (stock_quantity >= 0),
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    );

    CREATE TYPE order_status_enum AS ENUM (
        'pending',
        'payment_due',
        'paid',
        'failed',
        'shipped',
        'delivered',
        'cancelled'
    );

    CREATE TABLE orders (
        id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
        user_id UUID NOT NULL REFERENCES users(id),
        status order_status_enum NOT NULL DEFAULT 'pending',
        total_amount_cents INT NOT NULL CHECK (total_amount_cents >= 0),
        currency VARCHAR(3) NOT NULL DEFAULT 'USD',
        payment_gateway_txn_id TEXT,
        payment_gateway_client_data TEXT, -- For things like Stripe client_secret
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    );

    CREATE TABLE order_items (
        id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
        order_id UUID NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
        product_id UUID NOT NULL REFERENCES products(id),
        quantity INT NOT NULL CHECK (quantity > 0),
        price_at_purchase_cents INT NOT NULL CHECK (price_at_purchase_cents >= 0)
        -- No created_at/updated_at as items are typically immutable once order is placed
    );

    CREATE TABLE cart_items (
        id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
        user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        product_id UUID NOT NULL REFERENCES products(id),
        quantity INT NOT NULL CHECK (quantity > 0),
        added_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        UNIQUE (user_id, product_id) -- Ensure a user has only one cart entry per product
    );

    -- Optional: Triggers to update `updated_at` timestamps
    CREATE OR REPLACE FUNCTION trigger_set_timestamp()
    RETURNS TRIGGER AS $$
    BEGIN
      NEW.updated_at = NOW();
      RETURN NEW;
    END;
    $$ LANGUAGE plpgsql;

    CREATE TRIGGER set_user_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW
    EXECUTE FUNCTION trigger_set_timestamp();

    CREATE TRIGGER set_product_updated_at
    BEFORE UPDATE ON products
    FOR EACH ROW
    EXECUTE FUNCTION trigger_set_timestamp();

    CREATE TRIGGER set_order_updated_at
    BEFORE UPDATE ON orders
    FOR EACH ROW
    EXECUTE FUNCTION trigger_set_timestamp();
    ```
    Apply this schema to your database: `psql -U orka_user -d orka_ecommerce_db -f schema.sql`

### Configuration

1.  Navigate to the `examples/ecommerce_app` directory.
2.  Create a `.env` file by copying `.env.example` (if provided, otherwise create it):
    ```bash
    cp .env.example .env
    ```
    Or create `examples/ecommerce_app/.env` with the following content:
    ```env
    # .env for ecommerce_app example

    # Server Configuration
    SERVER_HOST=127.0.0.1
    SERVER_PORT=8081 # Or any other port

    # Database URL (Update with your credentials)
    DATABASE_URL="postgres://orka_user:your_strong_password@localhost:5432/orka_ecommerce_db"

    # Application Base URL (Optional, used by some email templates or links)
    # APP_BASE_URL="http://localhost:8081"

    # Mock Payment Provider IDs (Used by checkout pipeline)
    MOCK_PAYMENT_MAIN_ID="mock_main_acct_123"
    MOCK_PAYMENT_ALT_ID="mock_alt_acct_456"

    # Mock Email Sender
    MOCK_EMAIL_SENDER="noreply@orka-commerce.com"

    # Seed database on startup (true/false) - Placeholder, seeding logic not fully implemented
    SEED_DB=false

    # Logging Level (e.g., info, debug, trace, warn, error)
    RUST_LOG="info,sqlx=warn,ecommerce_app_server=debug,orka=debug"
    ```
3.  **Update `DATABASE_URL` in your `.env` file** with your actual PostgreSQL connection string.
4.  Adjust `RUST_LOG` as needed for desired logging verbosity.

### Running the Application

1.  Navigate to the root of the `orka` project (the directory containing the main `Cargo.toml` for the Orka workspace).
2.  Run the example:
    ```bash
    cargo run --example ecommerce_app_server
    ```
    (The name `ecommerce_app_server` comes from what you'd typically name the `[[bin]]` or `[[example]]` target in `Cargo.toml`. If your example's target name in `examples/ecommerce_app/Cargo.toml` is different, use that name.)

    If `ecommerce_app` is defined as an example target in the *workspace* `Cargo.toml`, you might run it as:
    ```bash
    cargo run -p ecommerce_app # Assuming Cargo.toml in examples/ecommerce_app defines a binary
    ```
    Or more commonly for examples:
    ```bash
    cargo run --example ecommerce_app # If Cargo.toml in examples/ecommerce_app has name="ecommerce_app"
    ```
    The exact command depends on how the `Cargo.toml` for the example is structured. If it's `name = "ecommerce_app_server"` in its `Cargo.toml`, then `cargo run -p ecommerce_app_server` from the workspace root or `cargo run` from within `examples/ecommerce_app` would work. Let's assume for the Orka project structure it's an example target:
    ```bash
    cargo run --example ecommerce_app
    ```
    *(Adjust the command based on your actual `Cargo.toml` target name for the example.)*

3.  The server should start, and you'll see logs in your console, typically indicating it's listening on `127.0.0.1:8081` (or the port you configured).

## API Endpoints (Examples)

(Assuming base path `/api/v1`)

*   `GET /api/v1/health`: Health check.
*   **Auth:**
    *   `POST /api/v1/auth/signup`: Register a new user.
        *   Body: `{"email": "user@example.com", "password": "password123"}`
    *   `POST /api/v1/auth/signin`: Sign in an existing user.
        *   Body: `{"email": "user@example.com", "password": "password123"}`
*   **Products:**
    *   `GET /api/v1/products`: List all products.
    *   `GET /api/v1/products/{product_id}`: Get a specific product.
*   **Cart:**
    *   `POST /api/v1/cart/add`: Add an item to the cart.
        *   Requires authentication (e.g., `X-User-ID` header for this mock: `X-User-ID: <user_uuid>`)
        *   Body: `{"product_id": "<product_uuid>", "quantity": 1}`
*   **Checkout:**
    *   `POST /api/v1/checkout`: Initiate the checkout process for the authenticated user.
        *   Requires authentication (e.g., `X-User-ID` header).
*   **Webhooks:**
    *   `POST /api/v1/webhooks/{source}`: Generic endpoint to receive webhooks.
        *   Example: `POST /api/v1/webhooks/mock_payment_gateway`
        *   Body: JSON payload from the webhook provider.
        *   (Optional) `Stripe-Signature` header (or similar) for signature verification.

### Example cURL Commands

*(Replace UUIDs and other placeholders with actual values after running the app and creating data.)*

**1. Signup:**
```bash
curl -X POST -H "Content-Type: application/json" \
-d '{"email": "test@example.com", "password": "password123"}' \
http://localhost:8081/api/v1/auth/signup
```
*(Note the user ID from the response for subsequent authenticated requests.)*

**2. Signin:**
```bash
curl -X POST -H "Content-Type: application/json" \
-d '{"email": "test@example.com", "password": "password123"}' \
http://localhost:8081/api/v1/auth/signin
```
*(Note the token from the response.)*

**3. List Products:**
```bash
curl http://localhost:8081/api/v1/products
```
*(Note a product ID from the response.)*

**4. Add to Cart (replace `<USER_UUID>` and `<PRODUCT_UUID>`):**
```bash
curl -X POST -H "Content-Type: application/json" \
-H "X-User-ID: <USER_UUID>" \
-d '{"product_id": "<PRODUCT_UUID>", "quantity": 2}' \
http://localhost:8081/api/v1/cart/add
```

**5. Start Checkout (replace `<USER_UUID>`):**
```bash
curl -X POST \
-H "X-User-ID: <USER_UUID>" \
http://localhost:8081/api/v1/checkout
```

**6. Mock Webhook (Payment Succeeded):**
```bash
curl -X POST -H "Content-Type: application/json" \
-d '{"event_type": "payment_succeeded", "data": {"order_id": "<ORDER_UUID_FROM_CHECKOUT>", "amount": 5000, "currency": "USD"}}' \
http://localhost:8081/api/v1/webhooks/mock_payment_gateway
```

## How Orka is Used

*   **`signup_pipeline.rs`**: Orchestrates user validation, existence check, password hashing, DB insertion, and welcome email sending.
*   **`signin_pipeline.rs`**: Handles input validation, user fetching, password verification, and (mock) token issuance.
*   **`cart_pipeline.rs`**: Manages adding items to a cart, including product validation and stock checks.
*   **`checkout_pipeline.rs`**: A more complex pipeline demonstrating:
    *   Initial order creation.
    *   **Conditional Scopes:** Dynamically selecting a payment processing sub-pipeline (`Pipeline<SData, AppError>`) based on routing logic. Factories (`pipelines/factories.rs`) create these payment sub-pipelines on demand.
    *   Updating order status based on payment outcome.
    *   Sending confirmation emails (optional step).
*   **`webhook_pipeline.rs`**:
    *   Handles initial webhook validation (e.g., signature).
    *   Uses **Conditional Scopes** to route events based on source and event type to different specialized sub-pipelines for processing. Extractors are used to parse the generic webhook payload into a context suitable for the sub-pipeline.

This example aims to provide a solid foundation for understanding how to apply Orka to real-world application workflows.