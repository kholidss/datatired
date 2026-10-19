// mongosh-compatible shell shim. Values cross the Rust bridge as Extended JSON:
// Rust -> JS via __revive, JS -> Rust via JSON.stringify (helper classes carry the EJSON shape).
"use strict";

class _ObjectId {
  constructor(h) { this.$oid = h === undefined ? __req("newOid", {}) : String(h); }
  toString() { return this.$oid; }
  toHexString() { return this.$oid; }
  equals(o) { return o != null && String(o) === this.$oid; }
  getTimestamp() { return new Date(parseInt(this.$oid.slice(0, 8), 16) * 1000); }
}
class _Long {
  constructor(v) { this.$numberLong = String(v); }
  valueOf() { return Number(this.$numberLong); }
  toString() { return this.$numberLong; }
  toNumber() { return Number(this.$numberLong); }
}
class _Decimal {
  constructor(v) { this.$numberDecimal = String(v); }
  valueOf() { return Number(this.$numberDecimal); }
  toString() { return this.$numberDecimal; }
}
class _Int32 {
  constructor(v) { this.$numberInt = String(v | 0); }
  valueOf() { return Number(this.$numberInt); }
  toString() { return this.$numberInt; }
}
class _Double {
  constructor(v) { this.$numberDouble = String(Number(v)); }
  valueOf() { return Number(this.$numberDouble); }
  toString() { return this.$numberDouble; }
}
class _Timestamp {
  constructor(t, i) {
    if (typeof t === "object" && t !== null) this.$timestamp = { t: t.t | 0, i: t.i | 0 };
    else this.$timestamp = { t: t | 0, i: i | 0 };
  }
}
class _Binary { constructor(b64, sub) { this.$binary = { base64: b64, subType: (sub || 0).toString(16).padStart(2, "0") }; } }
class _UUID { constructor(s) { this.$uuid = s === undefined ? __req("newUuid", {}) : String(s); } toString() { return this.$uuid; } }
class _Key { constructor(k) { this[k] = 1; } }

function ObjectId(h) { return new _ObjectId(h); }
ObjectId.isValid = (h) => typeof h === "string" && /^[0-9a-fA-F]{24}$/.test(h);
function NumberLong(v) { return new _Long(v); }
function Long(v) { return new _Long(v); }
Long.fromNumber = (v) => new _Long(Math.trunc(v));
Long.fromString = (v) => new _Long(v);
function NumberInt(v) { return new _Int32(v); }
function Int32(v) { return new _Int32(v); }
function Double(v) { return new _Double(v); }
function NumberDecimal(v) { return new _Decimal(v); }
function Decimal128(v) { return new _Decimal(v); }
Decimal128.fromString = (v) => new _Decimal(v);
function Timestamp(t, i) { return new _Timestamp(t, i); }
function BinData(sub, b64) { return new _Binary(b64, sub); }
function UUID(s) { return new _UUID(s); }
function MinKey() { return new _Key("$minKey"); }
function MaxKey() { return new _Key("$maxKey"); }
function ISODate(s) { return s === undefined ? new Date() : new Date(s); }

function __replacer(k, v) {
  const raw = this[k];
  if (raw instanceof Date) return isNaN(raw) ? null : { $date: raw.toISOString() };
  if (raw instanceof RegExp) return { $regularExpression: { pattern: raw.source, options: raw.flags.replace(/[gy]/g, "") } };
  if (typeof raw === "bigint") return { $numberLong: String(raw) };
  return v;
}
function __ejson(v) { return JSON.stringify(v === undefined ? null : v, __replacer); }

function __revive(k, v) {
  if (v === null || typeof v !== "object" || Array.isArray(v)) return v;
  const keys = Object.keys(v);
  if (keys.length === 1) {
    switch (keys[0]) {
      case "$oid": return new _ObjectId(v.$oid);
      case "$date": return new Date(typeof v.$date === "object" ? Number(v.$date.$numberLong) : v.$date);
      case "$numberLong": return new _Long(v.$numberLong);
      case "$numberDecimal": return new _Decimal(v.$numberDecimal);
      case "$uuid": return new _UUID(v.$uuid);
      case "$regularExpression": return new RegExp(v.$regularExpression.pattern, v.$regularExpression.options);
    }
  }
  return v;
}

function __req(op, args) {
  const r = JSON.parse(__native(__ejson({ op, ...args })), __revive);
  if (r.error !== undefined) throw new Error(r.error);
  return r.ok;
}

function print(...a) { __print(a.map((x) => (typeof x === "string" ? x : __ejson(x))).join(" ")); }
const printjson = (x) => __print(JSON.stringify(JSON.parse(__ejson(x)), null, 2));
const console = { log: print, info: print, warn: print, error: print, debug: print };

class Cursor {
  constructor(spec) { this._s = spec; this._buf = null; }
  sort(s) { this._s.sort = s; return this; }
  limit(n) { this._s.limit = n; return this; }
  skip(n) { this._s.skip = n; return this; }
  projection(p) { this._s.projection = p; return this; }
  project(p) { return this.projection(p); }
  hint(h) { this._s.hint = h; return this; }
  collation(c) { this._s.collation = c; return this; }
  batchSize() { return this; }
  maxTimeMS() { return this; }
  comment() { return this; }
  allowDiskUse() { return this; }
  noCursorTimeout() { return this; }
  readPref() { return this; }
  pretty() { return this; }
  toArray() { return __req("cursor", { spec: this._s }); }
  _fill() { if (this._buf === null) this._buf = this.toArray(); return this._buf; }
  hasNext() { return this._fill().length > 0; }
  next() { return this._fill().shift() ?? null; }
  tryNext() { return this.next(); }
  forEach(f) { this.toArray().forEach(f); }
  map(f) { return this.toArray().map(f); }
  count() { return __req("count", { spec: this._s }); }
  size() { return this.count(); }
  itcount() { return this.toArray().length; }
  explain(v) { return __req("explain", { spec: this._s, verbosity: v || "queryPlanner" }); }
  [Symbol.iterator]() { return this.toArray()[Symbol.iterator](); }
}

class Collection {
  constructor(db, name) { this._db = db; this._name = name; }
  _r(op, a) { return __req(op, { db: this._db, coll: this._name, ...a }); }
  getName() { return this._name; }
  getFullName() { return this._db + "." + this._name; }
  getDB() { return __dbProxy(this._db); }
  toString() { return this.getFullName(); }
  find(filter, projection) { return new Cursor({ db: this._db, coll: this._name, op: "find", filter: filter || {}, projection }); }
  findOne(filter, projection) { return this.find(filter, projection).limit(1).toArray()[0] ?? null; }
  aggregate(pipeline, options) {
    if (!Array.isArray(pipeline)) { pipeline = [...arguments]; options = undefined; }
    return new Cursor({ db: this._db, coll: this._name, op: "aggregate", pipeline, options });
  }
  insertOne(doc, options) { return this._r("insertOne", { doc, options }); }
  insertMany(docs, options) { return this._r("insertMany", { docs, options }); }
  insert(d) { return Array.isArray(d) ? this.insertMany(d) : this.insertOne(d); }
  updateOne(filter, update, options) { return this._r("updateOne", { filter, update, options }); }
  updateMany(filter, update, options) { return this._r("updateMany", { filter, update, options }); }
  update(filter, update, o) { return (o && o.multi) ? this.updateMany(filter, update, o) : this.updateOne(filter, update, o); }
  replaceOne(filter, doc, options) { return this._r("replaceOne", { filter, doc, options }); }
  deleteOne(filter, options) { return this._r("deleteOne", { filter: filter || {}, options }); }
  deleteMany(filter, options) { return this._r("deleteMany", { filter: filter || {}, options }); }
  remove(filter, o) { return (o === true || (o && o.justOne)) ? this.deleteOne(filter) : this.deleteMany(filter); }
  findOneAndUpdate(filter, update, options) { return this._r("findOneAndUpdate", { filter, update, options }); }
  findOneAndReplace(filter, doc, options) { return this._r("findOneAndReplace", { filter, doc, options }); }
  findOneAndDelete(filter, options) { return this._r("findOneAndDelete", { filter, options }); }
  countDocuments(filter, options) { return this._r("countDocuments", { filter: filter || {}, options }); }
  count(filter) { return this.countDocuments(filter); }
  estimatedDocumentCount() { return this._r("estimatedDocumentCount", {}); }
  distinct(key, filter) { return this._r("distinct", { key, filter: filter || {} }); }
  createIndex(keys, options) { return this._r("createIndex", { keys, options }); }
  createIndexes(list, options) { return list.map((k) => this.createIndex(k, options)); }
  ensureIndex(keys, options) { return this.createIndex(keys, options); }
  getIndexes() { return this._r("getIndexes", {}); }
  getIndexKeys() { return this.getIndexes().map((i) => i.key); }
  dropIndex(name) { return this._r("dropIndex", { name: typeof name === "string" ? name : undefined, keys: typeof name === "string" ? undefined : name }); }
  dropIndexes() { return this._r("dropIndexes", {}); }
  drop() { return this._r("drop", {}); }
  renameCollection(to, dropTarget) { return this._r("renameCollection", { to, dropTarget: !!dropTarget }); }
  stats() { return __dbProxy(this._db).runCommand({ collStats: this._name }); }
  bulkWrite(ops) {
    let r = { acknowledged: true, insertedCount: 0, matchedCount: 0, modifiedCount: 0, deletedCount: 0, upsertedCount: 0 };
    for (const op of ops) {
      const [k, a] = Object.entries(op)[0];
      if (k === "insertOne") { this.insertOne(a.document); r.insertedCount++; }
      else if (k === "updateOne" || k === "updateMany" || k === "replaceOne") {
        const x = k === "replaceOne" ? this.replaceOne(a.filter, a.replacement, a) : this[k](a.filter, a.update, a);
        r.matchedCount += x.matchedCount; r.modifiedCount += x.modifiedCount; r.upsertedCount += x.upsertedCount;
      } else if (k === "deleteOne" || k === "deleteMany") { r.deletedCount += this[k](a.filter).deletedCount; }
    }
    return r;
  }
}

function __collProxy(db, name) {
  return new Proxy(new Collection(db, name), {
    get(t, p) { return (p in t || typeof p === "symbol") ? t[p] : __collProxy(db, name + "." + p); },
  });
}

class Database {
  constructor(name) { this._name = name; }
  getName() { return this._name; }
  toString() { return this._name; }
  getCollection(n) { return __collProxy(this._name, n); }
  getSiblingDB(n) { return __dbProxy(n); }
  getMongo() { return { getDBNames: () => __req("listDatabases", {}).map((d) => d.name), getDB: (n) => __dbProxy(n) }; }
  getCollectionNames() { return __req("listCollections", { db: this._name, nameOnly: true }).map((c) => c.name).sort(); }
  getCollectionInfos(filter) { return __req("listCollections", { db: this._name, filter }); }
  createCollection(name, options) { return __req("createCollection", { db: this._name, name, options }); }
  createView(name, source, pipeline, options) { return this.createCollection(name, { ...options, viewOn: source, pipeline }); }
  runCommand(cmd) { return __req("runCommand", { db: this._name, cmd: typeof cmd === "string" ? { [cmd]: 1 } : cmd }); }
  adminCommand(cmd) { return __dbProxy("admin").runCommand(cmd); }
  dropDatabase() { return __req("dropDatabase", { db: this._name }); }
  stats() { return this.runCommand({ dbStats: 1 }); }
  version() { return this.adminCommand({ buildInfo: 1 }).version; }
  serverStatus() { return this.adminCommand({ serverStatus: 1 }); }
  hostInfo() { return this.adminCommand({ hostInfo: 1 }); }
  currentOp() { return this.adminCommand({ currentOp: 1 }); }
  getProfilingStatus() { return this.runCommand({ profile: -1 }); }
  aggregate(pipeline) { return new Cursor({ db: this._name, coll: null, op: "aggregate", pipeline }); }
}

function __dbProxy(name) {
  return new Proxy(new Database(name), {
    get(t, p) { return (p in t || typeof p === "symbol") ? t[p] : __collProxy(name, String(p)); },
  });
}

globalThis.__currentDb = "test";
Object.defineProperty(globalThis, "db", { get: () => __dbProxy(globalThis.__currentDb), configurable: true });

// Final value of a statement -> description for the Rust side.
function __describe(v) {
  if (v instanceof Cursor) return __ejson({ kind: "cursor", spec: v._s });
  if (v === undefined) return '{"kind":"undefined"}';
  if (v instanceof Collection || v instanceof Database) return __ejson({ kind: "value", value: String(v) });
  if (typeof v === "function") return __ejson({ kind: "value", value: "[Function " + (v.name || "anonymous") + "]" });
  return __ejson({ kind: "value", value: v });
}
// A statement's `.toArray()` left out by the Rust side: a cursor is shown page by page, anything
// else gets its toArray() back.
function __finish(v) { return v instanceof Cursor ? v : v.toArray(); }
// Evaluate a JS literal (filter/sort/cell value) to EJSON.
function __eval_literal(src) { return __ejson((0, eval)("(" + src + ")")); }
