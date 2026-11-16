export class Shape {
  radius = 1;
  area(): number {
    return Math.PI * this.radius ** 2;
  }
}
