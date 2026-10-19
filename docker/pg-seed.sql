-- Seed data for the PostgreSQL data source tests (database `datatired`).
-- psql -U postgres -c 'create database datatired' && psql -U postgres -d datatired -f pg-seed.sql
drop schema if exists public cascade;
drop schema if exists logs cascade;
create schema public;
create schema logs;

create table customers (
    id         serial primary key,
    name       text        not null,
    email      text        not null unique,
    age        integer,
    active     boolean     not null default true,
    tier       text,
    balance    numeric(10, 2),
    visits     bigint,
    created_at timestamptz not null default now(),
    address    jsonb,
    tags       text[],
    note       text
);
create index customers_city_age on customers ((address ->> 'city'), age desc);

insert into customers (name, email, age, active, tier, balance, visits, created_at, address, tags, note)
select 'Customer ' || i,
       'customer' || i || '@example.com',
       18 + i % 50,
       i % 3 <> 0,
       (array ['bronze', 'silver', 'gold'])[i % 3 + 1],
       i * 12.5,
       i * 1000,
       timestamptz '2025-01-01 10:30:00+00' + (i || ' days')::interval,
       jsonb_build_object('city', (array ['Jakarta', 'Bandung', 'Surabaya', 'Medan'])[i % 4 + 1], 'zip', (10000 + i)::text),
       case when i % 2 = 1 then array ['vip', 'newsletter'] else '{}' end,
       case when i % 5 = 0 then null else 'note ' || i end
from generate_series(1, 120) i;

create table products (
    sku          varchar(16) primary key,
    name         text           not null,
    price        numeric(8, 2)  not null,
    stock        integer        not null default 0,
    category     text,
    discontinued boolean        not null default false
);
insert into products values
    ('KB-01', 'Keyboard', 49.9, 120, 'peripheral', false),
    ('MS-01', 'Mouse', 19.9, 300, 'peripheral', false),
    ('MN-27', 'Monitor 27"', 299.0, 25, 'display', false),
    ('HD-01', 'Headset', 79.5, 0, 'audio', true),
    ('CB-USB', 'USB-C Cable', 9.99, 1000, 'accessory', false);

create table orders (
    id          bigserial primary key,
    customer_id integer     not null references customers (id),
    sku         varchar(16) not null references products (sku),
    qty         integer     not null check (qty > 0),
    total       numeric(10, 2),
    status      text        not null,
    ordered_at  date
);
insert into orders (customer_id, sku, qty, total, status, ordered_at)
select i % 120 + 1,
       (array ['KB-01', 'MS-01', 'MN-27', 'HD-01', 'CB-USB'])[i % 5 + 1],
       i % 4 + 1,
       round(i * 33.3, 2),
       (array ['pending', 'paid', 'shipped', 'cancelled'])[i % 4 + 1],
       date '2026-01-01' + i
from generate_series(1, 60) i;

create view paid_orders as select * from orders where status = 'paid';

create table logs.events (
    id   bigserial primary key,
    at   timestamp not null default now(),
    kind text,
    data json
);
insert into logs.events (at, kind, data) values
    ('2026-01-01 08:00:00', 'login', '{"user": 1}'),
    ('2026-01-01 08:05:00', 'logout', '{"user": 1}');
