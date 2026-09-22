import { useEffect, useState } from "react";
import { ApplicationClient } from "./ipc/ApplicationClient";

const applicationClient = new ApplicationClient();

/** Displays the initial ComposeNest desktop screen. */
export function App() {
  const [title, setTitle] = useState("ComposeNest");

  useEffect(() => {
    void applicationClient
      .getBootstrap(crypto.randomUUID())
      .then((bootstrap) => setTitle(bootstrap.applicationTitle))
      .catch(() => undefined);
  }, []);

  return (
    <main>
      <h1>{title}</h1>
      <p>Docker環境を管理する準備をしています。</p>
    </main>
  );
}
