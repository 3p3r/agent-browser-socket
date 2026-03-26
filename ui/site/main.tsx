import React from "react";
import ReactDOM from "react-dom/client";
import { LandingPage } from "./LandingPage";

class App extends React.Component {
  render() {
    return <LandingPage />;
  }
}

ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
