from pathlib import Path
from PIL import Image, ImageDraw

output = Path("src-tauri/icons")
output.mkdir(parents=True, exist_ok=True)
size = 512
image = Image.new("RGBA", (size, size), (0, 0, 0, 0))
draw = ImageDraw.Draw(image)
draw.rounded_rectangle((18, 18, 494, 494), radius=108, fill="#171717")
draw.rounded_rectangle((22, 22, 490, 490), radius=106, outline="#505050", width=5)
points = [(83, 266), (166, 266), (205, 194), (252, 340), (295, 235), (330, 266), (429, 266)]
draw.line(points, fill="#eeeeee", width=34, joint="curve")
for point in (points[0], points[-1]):
    draw.ellipse((point[0]-17, point[1]-17, point[0]+17, point[1]+17), fill="#eeeeee")

image.save(output / "icon.png")
for pixels, name in [(32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png")]:
    image.resize((pixels, pixels), Image.Resampling.LANCZOS).save(output / name)
image.save(output / "icon.ico", sizes=[(16,16),(32,32),(48,48),(64,64),(128,128),(256,256)])
