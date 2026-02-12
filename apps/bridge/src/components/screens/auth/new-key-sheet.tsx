import { Plus } from "lucide-solid";
import { createSignal } from "solid-js";
import { Button } from "../../ui/button";
import { Checkbox } from "../../ui/check-box";
import {
  Drawer,
  DrawerClose,
  DrawerContent,
  DrawerDescription,
  DrawerHeader,
  DrawerTitle,
  DrawerTrigger,
} from "../../ui/drawer";
import { Label } from "../../ui/label";
import { TextField, TextFieldInput, TextFieldLabel } from "../../ui/text-field";
const NewKeySheet = () => {
  const [name, setName] = createSignal("");

  return (
    <Drawer>
      <DrawerTrigger>
        <Button variant={"secondary"} onClick={() => {}} size={"sm"}>
          <Plus />
          New Key
        </Button>
      </DrawerTrigger>
      <DrawerContent>
        <DrawerHeader class="sr-only">
          <DrawerTitle>New API Key</DrawerTitle>
          <DrawerDescription>Generate a new API key for Cntrl Bridge.</DrawerDescription>
        </DrawerHeader>
        <div class="flex w-full flex-col gap-2 px-2">
          <p class="text-sm font-semibold">New API Key</p>
          <TextField>
            <TextFieldLabel class="text-neutral-500">Name</TextFieldLabel>
            <TextFieldInput
              value={name()}
              onInput={(e) => setName(e.currentTarget.value)}
            />
          </TextField>
          <div class="flex flex-col">
            <p class="text-sm font-medium text-neutral-500">Scopes</p>
            <div class="flex items-start space-x-2">
              <Checkbox id="terms1" />
              <Label for="terms1-input" class="leading-none">
                Read
              </Label>
            </div>
          </div>
        </div>
        <div class="flex w-full items-center justify-between p-2">
          <DrawerClose>
            <Button variant="ghost">Cancel</Button>
          </DrawerClose>
          <Button variant="secondary">Submit</Button>
        </div>
      </DrawerContent>
    </Drawer>
  );
};

export default NewKeySheet;
