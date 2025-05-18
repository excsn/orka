-- orka_project/examples/ecommerce_app/schema.sql

-- Ensure this script is idempotent if possible (e.g., using IF NOT EXISTS)
-- or instruct users to run it on a fresh database.

-- Drop tables if they exist (for easy re-creation during development)
-- In a real scenario, be cautious with DROP TABLE.
DROP TABLE IF EXISTS cart_items CASCADE;
DROP TABLE IF EXISTS order_items CASCADE;
DROP TABLE IF EXISTS orders CASCADE;
DROP TABLE IF EXISTS products CASCADE;
DROP TABLE IF EXISTS users CASCADE;
DROP TYPE IF EXISTS order_status_enum; -- Renamed from order_status to avoid conflict with potential built-in types

-- Create custom ENUM type for order status
CREATE TYPE order_status_enum AS ENUM ('pending', 'payment_due', 'paid', 'failed', 'shipped', 'delivered', 'cancelled');

-- Users Table
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email VARCHAR(255) UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Products Table
CREATE TABLE products (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(255) NOT NULL UNIQUE,
    description TEXT,
    price_cents INTEGER NOT NULL CHECK (price_cents >= 0),
    stock_quantity INTEGER NOT NULL DEFAULT 0 CHECK (stock_quantity >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Orders Table
CREATE TABLE orders (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status order_status_enum NOT NULL DEFAULT 'pending',
    total_amount_cents INTEGER NOT NULL CHECK (total_amount_cents >= 0),
    currency VARCHAR(3) NOT NULL DEFAULT 'usd',
    stripe_payment_intent_id TEXT UNIQUE,
    stripe_client_secret TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_orders_user_id ON orders(user_id);
CREATE INDEX idx_orders_stripe_payment_intent_id ON orders(stripe_payment_intent_id);

-- Order Items Table
CREATE TABLE order_items (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    order_id UUID NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
    product_id UUID NOT NULL REFERENCES products(id) ON DELETE RESTRICT, -- Prevent deleting product if in an order
    quantity INTEGER NOT NULL CHECK (quantity > 0),
    price_at_purchase_cents INTEGER NOT NULL CHECK (price_at_purchase_cents >= 0),
    UNIQUE (order_id, product_id)
);
CREATE INDEX idx_order_items_order_id ON order_items(order_id);

-- Cart Items Table
CREATE TABLE cart_items (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    product_id UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    quantity INTEGER NOT NULL CHECK (quantity > 0),
    added_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, product_id)
);
CREATE INDEX idx_cart_items_user_id ON cart_items(user_id);

-- Optional: Trigger function to update 'updated_at' columns automatically
CREATE OR REPLACE FUNCTION trigger_set_timestamp()
RETURNS TRIGGER AS $$
BEGIN
  NEW.updated_at = NOW();
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Apply trigger to tables that have an updated_at column
CREATE TRIGGER set_timestamp_users
BEFORE UPDATE ON users
FOR EACH ROW
EXECUTE FUNCTION trigger_set_timestamp();

CREATE TRIGGER set_timestamp_products
BEFORE UPDATE ON products
FOR EACH ROW
EXECUTE FUNCTION trigger_set_timestamp();

CREATE TRIGGER set_timestamp_orders
BEFORE UPDATE ON orders
FOR EACH ROW
EXECUTE FUNCTION trigger_set_timestamp();

-- Notify user script is complete
\echo 'Database schema created successfully.'
\echo 'You can now run the ecommerce_app server.'