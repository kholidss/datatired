// Seed data for local testing. Runs once on first container start.
const shop = db.getSiblingDB("shop");

const customers = [];
for (let i = 1; i <= 120; i++) {
  customers.push({
    _id: ObjectId(),
    name: `Customer ${i}`,
    email: `customer${i}@example.com`,
    age: 18 + (i % 50),
    active: i % 3 !== 0,
    tier: ["bronze", "silver", "gold"][i % 3],
    balance: NumberDecimal(`${(i * 12.5).toFixed(2)}`),
    visits: NumberLong(i * 1000),
    createdAt: new Date(Date.UTC(2025, i % 12, (i % 28) + 1, 10, 30)),
    address: { city: ["Jakarta", "Bandung", "Surabaya", "Medan"][i % 4], zip: `${10000 + i}` },
    tags: i % 2 ? ["vip", "newsletter"] : [],
    note: i % 5 === 0 ? null : `note ${i}`,
  });
}
shop.customers.insertMany(customers);
shop.customers.createIndex({ email: 1 }, { unique: true });
shop.customers.createIndex({ "address.city": 1, age: -1 });

const products = [
  { sku: "KB-01", name: "Keyboard", price: 49.9, stock: 120, category: "peripheral" },
  { sku: "MS-01", name: "Mouse", price: 19.9, stock: 300, category: "peripheral" },
  { sku: "MN-27", name: "Monitor 27\"", price: 299.0, stock: 25, category: "display" },
  { sku: "HD-01", name: "Headset", price: 79.5, stock: 0, category: "audio", discontinued: true },
  { sku: "CB-USB", name: "USB-C Cable", price: 9.99, stock: 1000, category: "accessory" },
];
shop.products.insertMany(products);

const orders = [];
for (let i = 1; i <= 60; i++) {
  const c = customers[i % customers.length];
  orders.push({
    customerId: c._id,
    items: [
      { sku: products[i % 5].sku, qty: (i % 4) + 1 },
      { sku: products[(i + 2) % 5].sku, qty: 1 },
    ],
    total: Math.round(i * 33.3 * 100) / 100,
    status: ["pending", "paid", "shipped", "cancelled"][i % 4],
    orderedAt: new Date(Date.UTC(2026, i % 12, (i % 28) + 1)),
  });
}
shop.orders.insertMany(orders);
shop.createView("paid_orders", "orders", [{ $match: { status: "paid" } }]);

const logs = db.getSiblingDB("logs");
logs.createCollection("events", { capped: true, size: 1048576 });
for (let i = 0; i < 30; i++) {
  logs.events.insertOne({ level: ["info", "warn", "error"][i % 3], msg: `event ${i}`, ts: new Date() });
}
