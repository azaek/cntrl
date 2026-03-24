import { Store } from "@tauri-apps/plugin-store";

let _store: Store | null = null;

async function getStore(): Promise<Store> {
    if (!_store) {
        _store = await Store.load("ui-state.json");
    }
    return _store;
}

export const uiStore = {
    async get<T>(key: string): Promise<T | null> {
        const store = await getStore();
        return (await store.get<T>(key)) ?? null;
    },

    async set(key: string, value: unknown): Promise<void> {
        const store = await getStore();
        await store.set(key, value);
        await store.save();
    },
};

// Typed helpers for known keys
export const isOnboardingCompleted = () =>
    uiStore.get<boolean>("onboarding_completed").then((v) => v === true);

export const completeOnboarding = () => uiStore.set("onboarding_completed", true);
