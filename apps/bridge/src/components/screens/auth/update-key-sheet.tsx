import { ApiKeySummary } from "../../../lib/auth";
import {
  Drawer,
  DrawerContent,
  DrawerDescription,
  DrawerHeader,
  DrawerTitle,
} from "../../ui/drawer";

const UpdateKeySheet = (props: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  apiKey: ApiKeySummary;
}) => {
  return (
    <Drawer open={props.open} onOpenChange={props.onOpenChange}>
      <DrawerContent>
        <DrawerHeader class="sr-only">
          <DrawerTitle>Update API Key</DrawerTitle>
          <DrawerDescription>Update an API key for Cntrl Bridge.</DrawerDescription>
        </DrawerHeader>
        <UpdateKeySheetScreen apiKey={props.apiKey} />
      </DrawerContent>
    </Drawer>
  );
};

const UpdateKeySheetScreen = (props: { apiKey: ApiKeySummary }) => {
  return (
    <div>
      <h1>Update Key {props.apiKey.name}</h1>
    </div>
  );
};

export default UpdateKeySheet;
