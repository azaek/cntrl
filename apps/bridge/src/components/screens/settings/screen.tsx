import { useApp } from "../../../context/app-context";

const SettingsScreen = () => {
  const [store, actions] = useApp();
  return <div>Settings</div>;
};

export default SettingsScreen;
