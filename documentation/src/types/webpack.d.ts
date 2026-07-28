declare namespace NodeJS {
  interface Require {
    context(
      directory: string,
      includeSubdirectories: boolean,
      filter: RegExp,
    ): {
      keys(): string[];
      (id: string): any;
    };
  }
}
