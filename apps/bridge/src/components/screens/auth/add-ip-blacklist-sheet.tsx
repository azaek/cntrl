import { Plus } from "lucide-solid";
import { createSignal } from "solid-js";
import { useApp } from "../../../context/app-context";
import { addBlockedIp, getAuthInfo } from "../../../lib/auth";
import { Button } from "../../ui/button";
import {
    Drawer,
    DrawerClose,
    DrawerContent,
    DrawerDescription,
    DrawerHeader,
    DrawerTitle,
    DrawerTrigger,
} from "../../ui/drawer";
import { TextField, TextFieldInput, TextFieldLabel } from "../../ui/text-field";
const AddIpBlacklistSheet = () => {
    const [open, setOpen] = createSignal(false);

    return (
        <Drawer open={open()} onOpenChange={setOpen}>
            <DrawerTrigger>
                <Button variant={"secondary"} onClick={() => {}} size={"sm"}>
                    <Plus />
                    Add IP
                </Button>
            </DrawerTrigger>
            <DrawerContent>
                <DrawerHeader class="sr-only">
                    <DrawerTitle>Blacklist IP Address</DrawerTitle>
                    <DrawerDescription>
                        Block IP address from accessing bridge
                    </DrawerDescription>
                </DrawerHeader>
                <AddIpWhiteListScreen onClose={() => setOpen(false)} />
            </DrawerContent>
        </Drawer>
    );
};

const AddIpWhiteListScreen = (props: { onClose: () => void }) => {
    const [ip, setIp] = createSignal("");
    const [_, action] = useApp();

    const handleAdd = async () => {
        await addBlockedIp(ip());
        const stats = await getAuthInfo();
        action.setAuth(stats);
        setIp("");
        props.onClose();
    };

    const isValid = () => {
        return ip().length > 0;
    };

    return (
        <div class="mt-3 flex w-full flex-col px-3 pb-3">
            <p class="text-sm font-semibold">Blacklist IP Address</p>
            <p class="text-muted-foreground text-xs">
                Block an IP address from accessing the Bridge.
            </p>
            <TextField data-corvu-no-drag class="mt-2">
                <TextFieldLabel class="mb-1 text-neutral-500">IP Address</TextFieldLabel>
                <TextFieldInput
                    value={ip()}
                    onInput={(e) => setIp(e.currentTarget.value)}
                    placeholder={"X.X.X.X"}
                />
            </TextField>
            <div data-corvu-no-drag class="flex w-full items-center justify-between pt-2">
                <DrawerClose>
                    <Button variant="ghost">Cancel</Button>
                </DrawerClose>
                <Button disabled={!isValid()} onClick={handleAdd} variant="secondary">
                    Block IP
                </Button>
            </div>
        </div>
    );
};

export default AddIpBlacklistSheet;
