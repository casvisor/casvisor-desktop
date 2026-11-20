import { getCurrentWindow } from "@tauri-apps/api/window";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import Banner from "./Banner";
import "./index.css";

const isBanner = getCurrentWindow().label === "banner";
if (isBanner) {
  document.body.classList.add("banner");
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>{isBanner ? <Banner /> : <App />}</StrictMode>,
);
