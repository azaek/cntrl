import type { BridgePersistence, StoredBridge } from "@cntrl-pw/sdk";

const DB_NAME = "cntrl-bridges";
const STORE_NAME = "bridges";
const DB_VERSION = 1;

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        db.createObjectStore(STORE_NAME, { keyPath: "id" });
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function tx(db: IDBDatabase, mode: IDBTransactionMode): IDBObjectStore {
  return db.transaction(STORE_NAME, mode).objectStore(STORE_NAME);
}

export function createIndexedDbPersistence(): BridgePersistence {
  return {
    async load() {
      const db = await openDb();
      return new Promise<StoredBridge[]>((resolve, reject) => {
        const request = tx(db, "readonly").getAll();
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
      });
    },

    async onBridgeAdd(bridge) {
      const db = await openDb();
      return new Promise<void>((resolve, reject) => {
        const request = tx(db, "readwrite").put(bridge);
        request.onsuccess = () => resolve();
        request.onerror = () => reject(request.error);
      });
    },

    async onBridgeRemove(id) {
      const db = await openDb();
      return new Promise<void>((resolve, reject) => {
        const request = tx(db, "readwrite").delete(id);
        request.onsuccess = () => resolve();
        request.onerror = () => reject(request.error);
      });
    },

    async onBridgeUpdate(id, updates) {
      const db = await openDb();
      const store = tx(db, "readwrite");
      return new Promise<void>((resolve, reject) => {
        const getReq = store.get(id);
        getReq.onsuccess = () => {
          const existing = getReq.result;
          if (!existing) {
            reject(new Error(`Bridge ${id} not found`));
            return;
          }
          const putReq = store.put({ ...existing, ...updates, id });
          putReq.onsuccess = () => resolve();
          putReq.onerror = () => reject(putReq.error);
        };
        getReq.onerror = () => reject(getReq.error);
      });
    },
  };
}
