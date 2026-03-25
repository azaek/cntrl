/* @refresh reload */
import { render } from "solid-js/web";
import App from "./App.tsx";
import "./index.css";

const root = document.getElementById("root");

render(() => <App />, root!);

const splash = document.getElementById("splash");
if (splash) {
  splash.classList.add("hidden");
  setTimeout(() => splash.remove(), 200);
}
