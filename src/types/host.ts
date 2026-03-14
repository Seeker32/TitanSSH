export interface HostConfig {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  auth_type: AuthType;
  password?: string;
  private_key_path?: string;
  passphrase?: string;
  remark?: string;
}

export enum AuthType {
  Password = "Password",
  PrivateKey = "PrivateKey",
}
