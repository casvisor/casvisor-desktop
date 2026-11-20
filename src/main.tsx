import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import Banner from "./Banner";
import "./index.css";

const isBanner = window.location.hash === "#banner";
if (isBanner) {
  document.body.classList.add("banner");
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>{isBanner ? <Banner /> : <App />}</StrictMode>,
);
