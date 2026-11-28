import { MotionConfig } from "motion/react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./theme/tokens.css";
import "./index.css";
import { ThemeProvider } from "./theme/ThemeProvider";
import { ToastProvider } from "./components/common/Toast";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <MotionConfig reducedMotion="user">
      <ThemeProvider>
        <ToastProvider>
          <App />
        </ToastProvider>
      </ThemeProvider>
    </MotionConfig>
  </StrictMode>,
);
