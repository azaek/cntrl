import { Plus } from "lucide-solid";
import { Button } from "../../ui/button";
import {
  Drawer,
  DrawerContent,
  DrawerDescription,
  DrawerHeader,
  DrawerTitle,
  DrawerTrigger,
} from "../../ui/drawer";
import NewKeySheetScreen from "./new-key-sheet.screen";
const NewKeySheet = () => {
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
        <NewKeySheetScreen />
      </DrawerContent>
    </Drawer>
  );
};

export default NewKeySheet;
